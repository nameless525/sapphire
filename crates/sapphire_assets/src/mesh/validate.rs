//! The invariants a cooked mesh keeps beyond what bytecheck proves: the ones the GPU upload
//! and a skinning pass rely on.

use sapphire_serialize::cooked::CookedError;

use super::{ArchivedMesh, ArchivedPositions, ArchivedSkin, ArchivedVertexStreams, Bounds};

/// A cooked mesh that can't be read, or breaks an invariant of the format.
#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("the streams hold {positions} positions, {shading} shading and {surface} surface elements")]
    StreamLengths { positions: usize, shading: usize, surface: usize },
    #[error("{0} vertices are more than u32 indices address")]
    TooManyVertices(usize),
    #[error("bounds are not finite or not ordered (min <= max)")]
    InvalidBounds,
    #[error("UV bounds are not finite or not ordered (min <= max)")]
    InvalidUvBounds,
    #[error("vertex {vertex} has a non-finite position")]
    NonFinitePosition { vertex: usize },
    #[error("vertex {vertex} lies outside the mesh's bounds")]
    OutsideBounds { vertex: usize },
    #[error("vertex {vertex} sets the reserved position component")]
    ReservedNotZero { vertex: usize },
    #[error("index count {0} is not a multiple of 3")]
    NotTriangles(usize),
    #[error("index {index} at position {position} is out of range for {vertex_count} vertices")]
    IndexOutOfRange { position: usize, index: u32, vertex_count: usize },
    #[error("material {material} has a non-finite parameter")]
    NonFiniteMaterial { material: usize },
    #[error("the mesh has no LODs")]
    NoLods,
    #[error(
        "LOD {lod} reads {found} vertices; LOD0 reads all {vertex_count} and later LODs no more than the one before"
    )]
    LodVertexCount { lod: usize, found: u32, vertex_count: usize },
    #[error("LOD {lod} has an error that is negative, not finite or below the previous LOD's")]
    LodError { lod: usize },
    #[error("LOD {lod} submesh {submesh} does not cover whole triangles")]
    SubmeshNotTriangles { lod: usize, submesh: usize },
    #[error("LOD {lod} submesh {submesh} covers indices outside 0..{index_count}")]
    SubmeshOutOfRange { lod: usize, submesh: usize, index_count: usize },
    #[error("LOD {lod} submesh {submesh} references missing material {material}")]
    MissingMaterial { lod: usize, submesh: usize, material: u32 },
    #[error("LOD {lod} index {index} at position {position} is outside its {vertex_count}-vertex prefix")]
    OutsideLodPrefix { lod: usize, position: usize, index: u32, vertex_count: u32 },
    #[error("the skin has {0} joints or binds; u16 indices address at most 65 536")]
    TooManyJoints(usize),
    #[error("joint {joint}'s parent {parent} does not come before it")]
    JointOrder { joint: usize, parent: u16 },
    #[error("joint {joint} has a non-finite rest transform")]
    NonFiniteJoint { joint: usize },
    #[error("skin bind {bind} follows missing joint {joint}")]
    MissingJoint { bind: usize, joint: u16 },
    #[error("skin bind {bind} has a non-finite inverse bind matrix")]
    NonFiniteBind { bind: usize },
    #[error("the skin has influences for {found} vertices, not {vertex_count}")]
    InfluenceCount { found: usize, vertex_count: usize },
    #[error("vertex {vertex} is influenced by palette entry {bind}, beyond the {binds} the skin has")]
    MissingBind { vertex: usize, bind: u16, binds: usize },
    #[error("vertex {vertex}'s weights sum to {sum}/255")]
    WeightSum { vertex: usize, sum: u32 },
    #[error("morph target {target} does not start where the one before ends, or runs past the deltas")]
    MorphRange { target: usize },
    #[error("{0} morph deltas belong to no target")]
    UnownedDeltas(usize),
    #[error("morph target {target} has a non-finite default weight")]
    NonFiniteWeight { target: usize },
    #[error(
        "morph target {target} delta {delta}: vertex {vertex} is out of range or not after the previous delta's"
    )]
    MorphVertex { target: usize, delta: usize, vertex: u32 },
    #[error("morph target {target} delta {delta} is not finite")]
    NonFiniteDelta { target: usize, delta: usize },
}

/// Joint and bind indices are u16.
const MAX_JOINTS: usize = 1 << 16;

impl ArchivedMesh {
    /// Checks every invariant in [`MeshError`]. [`Mesh::read`](super::Mesh::read) calls it.
    pub fn validate(&self) -> Result<(), MeshError> {
        let vertex_count = self.streams.validate()?;
        let index_count = self.indices.len();
        if !index_count.is_multiple_of(3) {
            return Err(MeshError::NotTriangles(index_count));
        }
        if let Some((position, index)) =
            self.indices.iter().map(|i| i.to_native()).enumerate().find(|&(_, i)| i as usize >= vertex_count)
        {
            return Err(MeshError::IndexOutOfRange { position, index, vertex_count });
        }
        if let Some(material) = self.materials.iter().position(|m| {
            [&m.base_color, &m.shade_color, &m.outline_color, &m.params]
                .iter()
                .any(|v| v.iter().any(|c| !c.to_native().is_finite()))
        }) {
            return Err(MeshError::NonFiniteMaterial { material });
        }
        self.validate_lods(vertex_count)?;
        if let Some(skin) = self.skin.as_ref() {
            skin.validate(vertex_count)?;
        }
        self.validate_morphs(vertex_count)
    }

    fn validate_lods(&self, vertex_count: usize) -> Result<(), MeshError> {
        if self.lods.is_empty() {
            return Err(MeshError::NoLods);
        }
        let index_count = self.indices.len();
        let mut previous: Option<(u32, f32)> = None;
        for (lod, level) in self.lods.iter().enumerate() {
            let (found, error) = (level.vertex_count.to_native(), level.error.to_native());
            let too_many = match previous {
                None => found as usize != vertex_count,
                Some((previous_count, _)) => found > previous_count,
            };
            if too_many {
                return Err(MeshError::LodVertexCount { lod, found, vertex_count });
            }
            if !(error.is_finite() && error >= 0.0) || previous.is_some_and(|(_, previous)| error < previous)
            {
                return Err(MeshError::LodError { lod });
            }
            for (submesh, range) in level.submeshes.iter().enumerate() {
                let (first, count) = (range.first_index.to_native(), range.index_count.to_native());
                if !first.is_multiple_of(3) || !count.is_multiple_of(3) {
                    return Err(MeshError::SubmeshNotTriangles { lod, submesh });
                }
                let end = (first as usize).checked_add(count as usize).filter(|&end| end <= index_count);
                let Some(end) = end else {
                    return Err(MeshError::SubmeshOutOfRange { lod, submesh, index_count });
                };
                let material = range.material.to_native();
                if material as usize >= self.materials.len() {
                    return Err(MeshError::MissingMaterial { lod, submesh, material });
                }
                let indices = &self.indices[first as usize..end];
                if let Some(offset) = indices.iter().position(|i| i.to_native() >= found) {
                    let index = indices[offset].to_native();
                    let position = first as usize + offset;
                    return Err(MeshError::OutsideLodPrefix { lod, position, index, vertex_count: found });
                }
            }
            previous = Some((found, error));
        }
        Ok(())
    }

    fn validate_morphs(&self, vertex_count: usize) -> Result<(), MeshError> {
        let mut next = 0usize;
        for (target, morph) in self.morph_targets.iter().enumerate() {
            let (first, count) =
                (morph.first_delta.to_native() as usize, morph.delta_count.to_native() as usize);
            if first != next || first.checked_add(count).is_none_or(|end| end > self.morph_deltas.len()) {
                return Err(MeshError::MorphRange { target });
            }
            if !morph.default_weight.to_native().is_finite() {
                return Err(MeshError::NonFiniteWeight { target });
            }
            let mut last_vertex = None;
            for (delta, value) in self.morph_deltas[first..first + count].iter().enumerate() {
                let vertex = value.vertex();
                if vertex as usize >= vertex_count || last_vertex.is_some_and(|last| vertex <= last) {
                    return Err(MeshError::MorphVertex { target, delta, vertex });
                }
                if !value.position().iter().chain(&value.normal()).all(|c| c.is_finite()) {
                    return Err(MeshError::NonFiniteDelta { target, delta });
                }
                last_vertex = Some(vertex);
            }
            next = first + count;
        }
        match self.morph_deltas.len() - next {
            0 => Ok(()),
            unowned => Err(MeshError::UnownedDeltas(unowned)),
        }
    }
}

impl ArchivedVertexStreams {
    /// Checks the streams on their own and returns their vertex count.
    fn validate(&self) -> Result<usize, MeshError> {
        let (positions, shading, surface) = (self.len(), self.shading.len(), self.surface.len());
        if positions != shading || positions != surface {
            return Err(MeshError::StreamLengths { positions, shading, surface });
        }
        if u32::try_from(positions).is_err() {
            return Err(MeshError::TooManyVertices(positions));
        }
        if !self.bounds.to_native().is_valid() {
            return Err(MeshError::InvalidBounds);
        }
        if !self.uv_bounds.to_native().is_valid() {
            return Err(MeshError::InvalidUvBounds);
        }
        match &self.positions {
            ArchivedPositions::Quantized(values) => {
                if let Some(vertex) = values.iter().position(|p| p.reserved() != 0) {
                    return Err(MeshError::ReservedNotZero { vertex });
                }
            }
            ArchivedPositions::Float(values) => {
                if let Some(vertex) = values.iter().position(|p| !p.get().iter().all(|c| c.is_finite())) {
                    return Err(MeshError::NonFinitePosition { vertex });
                }
                // Quantized positions lie inside the bounds by construction; f32 ones must too,
                // since culling and light lists trust the bounds.
                let Bounds { min, max } = self.bounds.to_native();
                let inside = |p: [f32; 3]| (0..3).all(|axis| (min[axis]..=max[axis]).contains(&p[axis]));
                if let Some(vertex) = values.iter().position(|p| !inside(p.get())) {
                    return Err(MeshError::OutsideBounds { vertex });
                }
            }
        }
        Ok(positions)
    }
}

impl ArchivedSkin {
    fn validate(&self, vertex_count: usize) -> Result<(), MeshError> {
        let (joints, binds) = (self.joints.len(), self.binds.len());
        if joints > MAX_JOINTS || binds > MAX_JOINTS {
            return Err(MeshError::TooManyJoints(joints.max(binds)));
        }
        for (index, joint) in self.joints.iter().enumerate() {
            if let Some(parent) = joint.parent.as_ref().map(|parent| parent.to_native())
                && parent as usize >= index
            {
                return Err(MeshError::JointOrder { joint: index, parent });
            }
            let rest = &joint.rest;
            let finite = rest.translation.iter().chain(&rest.rotation).chain(&rest.scale);
            if !finite.map(|c| c.to_native()).all(f32::is_finite) {
                return Err(MeshError::NonFiniteJoint { joint: index });
            }
        }
        for (bind, entry) in self.binds.iter().enumerate() {
            let joint = entry.joint.to_native();
            if joint as usize >= joints {
                return Err(MeshError::MissingJoint { bind, joint });
            }
            if !entry.inverse_bind_matrix.iter().flatten().all(|c| c.to_native().is_finite()) {
                return Err(MeshError::NonFiniteBind { bind });
            }
        }
        let found = self.influences.len();
        if found != vertex_count {
            return Err(MeshError::InfluenceCount { found, vertex_count });
        }
        for vertex in 0..vertex_count {
            let influence = self.influences.get(vertex).expect("one influence per vertex");
            // Unused slots are read too (skinning blends all four, branch-free), so every
            // index must name a palette entry.
            if let Some(&bind) = influence.binds.iter().find(|&&bind| bind as usize >= binds) {
                return Err(MeshError::MissingBind { vertex, bind, binds });
            }
            let sum = influence.weights.iter().map(|&w| u32::from(w)).sum();
            if sum != 255 {
                return Err(MeshError::WeightSum { vertex, sum });
            }
        }
        Ok(())
    }
}
