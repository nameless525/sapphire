//! Morph targets (blendshapes): sparse per-vertex deltas, named for the face rigs and
//! animation clips that drive them.

use bytemuck::{Pod, Zeroable};

use super::encoding::{f16_bits, from_f16_bits, pack_u16, packed, unpack_u16};

/// One morph target. Its deltas are `Mesh::morph_deltas[first_delta..][..delta_count]`, by
/// increasing vertex, and list only the vertices it moves. The morphed vertex is
/// `base + Σ weight × delta` over the active targets.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct MorphTarget {
    /// The source's name for the shape (glTF `targetNames`), which expressions and clips use.
    pub name: String,
    /// The weight the source applies at rest (glTF `weights`), usually 0.
    pub default_weight: f32,
    pub first_delta: u32,
    pub delta_count: u32,
}

packed!(
    /// One vertex's displacement in one morph target: the vertex index (u32), then the position
    /// and normal deltas as binary16 × 3 each. 16 bytes, one `vec4<u32>` for a skinning
    /// shader (`unpack2x16float`).
    ///
    /// Half floats suit deltas, which range from sub-millimeter face shapes to body-scale
    /// corrective shapes: a fixed-point range sized for one loses the other, while binary16
    /// keeps 11 significant bits at every magnitude (a 5 cm delta within 15 µm), up to
    /// 65 504 m.
    MorphDelta,
    16
);

impl MorphDelta {
    pub fn new(vertex: u32, position: [f32; 3], normal: [f32; 3]) -> Self {
        let halves = [position[0], position[1], position[2], normal[0], normal[1], normal[2]].map(f16_bits);
        let mut bytes = [0; 16];
        bytes[..4].copy_from_slice(&vertex.to_le_bytes());
        bytes[4..].copy_from_slice(&pack_u16::<6, 12>(halves));
        Self::from_bytes(bytes)
    }

    pub fn vertex(self) -> u32 {
        u32::from_le_bytes(*self.to_bytes().first_chunk().expect("16 bytes hold 4"))
    }

    pub fn position(self) -> [f32; 3] {
        let halves = self.halves();
        [halves[0], halves[1], halves[2]].map(from_f16_bits)
    }

    pub fn normal(self) -> [f32; 3] {
        let halves = self.halves();
        [halves[3], halves[4], halves[5]].map(from_f16_bits)
    }

    /// The same delta for another vertex index (after a vertex reorder).
    pub fn with_vertex(self, vertex: u32) -> Self {
        let mut bytes = self.to_bytes();
        bytes[..4].copy_from_slice(&vertex.to_le_bytes());
        Self::from_bytes(bytes)
    }

    fn halves(self) -> [u16; 6] {
        unpack_u16(*self.to_bytes().last_chunk::<12>().expect("16 bytes hold 12"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_keep_their_vertex_and_both_vectors() {
        let delta = MorphDelta::new(70_000, [0.05, -0.0004, 1.5], [0.0, 1.0, -0.25]);
        assert_eq!(delta.vertex(), 70_000);
        assert_eq!(delta.normal(), [0.0, 1.0, -0.25], "exact in binary16");
        let position = delta.position();
        for (decoded, source) in position.iter().zip([0.05f32, -0.0004, 1.5]) {
            assert!((decoded - source).abs() <= source.abs() * 2f32.powi(-11), "{position:?}");
        }
        assert_eq!(delta.with_vertex(3).vertex(), 3);
        assert_eq!(delta.with_vertex(3).position(), position);
        assert_eq!(&delta.to_bytes()[4..6], &f16_bits(0.05).to_le_bytes());
    }
}
