//! Skins: the skeleton a mesh deforms with, its skinning palette and each vertex's
//! influences.

use bytemuck::{Pod, Zeroable};

use super::encoding::{pack_u16, packed, unpack_u16};

/// Joint influences each vertex keeps.
pub const MAX_INFLUENCES: usize = 4;

/// Palettes up to this size use [`Influences::U8`].
pub const MAX_U8_PALETTE: usize = 256;

/// A skinned mesh's skeleton, skinning palette and per-vertex influences. The positions and
/// normals of a skinned mesh are in bind space: what the palette multiplies.
///
/// - **Skeleton** ([`Self::joints`]): every joint in depth-first order, so parents come
///   before their children, with names and the rest pose as local transforms. That is the
///   shape of ozz-animation's runtime skeleton (spec §5.13): the animation cook maps its
///   tracks onto these joints by name.
/// - **Palette** ([`Self::binds`]): the matrices a skinning pass blends. Entry `i` is
///   `model_from_joint(binds[i].joint) × binds[i].inverse_bind_matrix`, with the joint's
///   model-space transform taken from the animated pose. In the rest pose every entry of a
///   mesh exported in its rest pose is the identity. A joint appears once per distinct
///   inverse bind matrix, and joints nothing is bound to (helpers, ancestors) not at all.
/// - **Influences** ([`Self::influences`]): per vertex, up to [`MAX_INFLUENCES`] palette
///   entries and their weights.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Skin {
    pub joints: Vec<Joint>,
    pub binds: Vec<SkinBind>,
    pub influences: Influences,
}

/// One joint of a [`Skin`]'s skeleton.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct Joint {
    /// The source node's name, which animation clips and attachments find the joint by.
    pub name: String,
    /// The parent's index, always lower than this joint's; `None` for a root.
    pub parent: Option<u16>,
    /// The rest pose, relative to the parent (to model space for a root).
    pub rest: JointTransform,
}

/// A joint's local transform: scale, then rotation, then translation.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct JointTransform {
    pub translation: [f32; 3],
    /// A unit quaternion, `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl JointTransform {
    pub const IDENTITY: Self =
        Self { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0], scale: [1.0; 3] };
}

/// One skinning-palette entry: the joint it follows, and the matrix that takes bind-space
/// positions into that joint's space.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct SkinBind {
    /// An index into [`Skin::joints`].
    pub joint: u16,
    /// Column-major, as in glTF.
    pub inverse_bind_matrix: [[f32; 4]; 4],
}

/// Every vertex's influences, in one of two widths of palette index.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub enum Influences {
    /// u8 palette indices, for palettes of up to [`MAX_U8_PALETTE`] entries. Characters
    /// within the content budget (at most 200 bones, spec §3.4) always fit.
    U8(Vec<Influence8>),
    /// u16 palette indices, for larger palettes.
    U16(Vec<Influence16>),
}

/// One vertex's influences, unpacked: palette indices and unorm8 weights.
///
/// Packed influences are ordered heaviest first and their weights sum to exactly 255 (1.0);
/// unused slots have weight 0 and index 0.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VertexInfluence {
    pub binds: [u16; MAX_INFLUENCES],
    pub weights: [u8; MAX_INFLUENCES],
}

packed!(
    /// A vertex's influences with u8 palette indices: four indices, then four unorm8 weights.
    /// 8 bytes, two u32 words for a skinning shader (`unpack4xU8`, `unpack4x8unorm`).
    Influence8,
    8
);

packed!(
    /// A vertex's influences with u16 palette indices: four little-endian indices, then four
    /// unorm8 weights. 12 bytes, three u32 words.
    Influence16,
    12
);

impl Influences {
    /// Packs `influences` (one per vertex) with the narrowest index that addresses a palette of
    /// `palette_len` entries.
    ///
    /// # Panics
    ///
    /// If an index doesn't fit u8 though `palette_len` is at most [`MAX_U8_PALETTE`].
    pub fn pack(influences: &[VertexInfluence], palette_len: usize) -> Self {
        if palette_len <= MAX_U8_PALETTE {
            Self::U8(
                influences
                    .iter()
                    .map(|influence| {
                        let binds =
                            influence.binds.map(|bind| u8::try_from(bind).expect("palette index fits u8"));
                        let mut bytes = [0; 8];
                        bytes[..4].copy_from_slice(&binds);
                        bytes[4..].copy_from_slice(&influence.weights);
                        Influence8::from_bytes(bytes)
                    })
                    .collect(),
            )
        } else {
            Self::U16(
                influences
                    .iter()
                    .map(|influence| {
                        let mut bytes = [0; 12];
                        bytes[..8].copy_from_slice(&pack_u16::<4, 8>(influence.binds));
                        bytes[8..].copy_from_slice(&influence.weights);
                        Influence16::from_bytes(bytes)
                    })
                    .collect(),
            )
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::U8(influences) => influences.len(),
            Self::U16(influences) => influences.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The influences of `vertex`, unpacked.
    pub fn get(&self, vertex: usize) -> Option<VertexInfluence> {
        match self {
            Self::U8(influences) => influences.get(vertex).map(|i| i.unpack()),
            Self::U16(influences) => influences.get(vertex).map(|i| i.unpack()),
        }
    }
}

impl ArchivedInfluences {
    pub fn len(&self) -> usize {
        match self {
            Self::U8(influences) => influences.len(),
            Self::U16(influences) => influences.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The influences of `vertex`, unpacked.
    pub fn get(&self, vertex: usize) -> Option<VertexInfluence> {
        match self {
            Self::U8(influences) => influences.get(vertex).map(|i| i.unpack()),
            Self::U16(influences) => influences.get(vertex).map(|i| i.unpack()),
        }
    }

    /// The stream's bytes, as a skinning pass reads them.
    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::U8(influences) => bytemuck::cast_slice(influences.as_slice()),
            Self::U16(influences) => bytemuck::cast_slice(influences.as_slice()),
        }
    }
}

impl Influence8 {
    pub fn unpack(self) -> VertexInfluence {
        let [binds, weights]: [[u8; 4]; 2] = bytemuck::cast(self.to_bytes());
        VertexInfluence { binds: binds.map(u16::from), weights }
    }
}

impl Influence16 {
    pub fn unpack(self) -> VertexInfluence {
        let bytes = self.to_bytes();
        let binds = unpack_u16(*bytes.first_chunk::<8>().expect("12 bytes hold 8"));
        VertexInfluence { binds, weights: *bytes.last_chunk::<4>().expect("12 bytes hold 4") }
    }
}

/// Quantizes non-negative weights to unorm8 that sum to exactly 255, by largest remainder: each
/// weight is normalized and rounded down, and the units left over go to the weights that lost
/// the most, earlier slots first on ties. Negative and NaN weights count as zero, and zero
/// weights stay zero; if nothing is left, slot 0 takes the whole weight.
pub fn quantize_weights(weights: [f32; MAX_INFLUENCES]) -> [u8; MAX_INFLUENCES] {
    let sum: f32 = weights.iter().map(|w| w.max(0.0)).sum();
    if !(sum.is_finite() && sum > 0.0) {
        return [255, 0, 0, 0];
    }
    let scaled = weights.map(|w| w.max(0.0) / sum * 255.0);
    let mut quantized = scaled.map(|w| w.floor().min(255.0) as u8);
    let total: u32 = quantized.iter().map(|&q| u32::from(q)).sum();
    let mut order: [usize; MAX_INFLUENCES] = std::array::from_fn(|i| i);
    // Stable, so ties keep slot order.
    order.sort_by(|&a, &b| (scaled[b] - scaled[b].floor()).total_cmp(&(scaled[a] - scaled[a].floor())));
    let mut left = 255u32.saturating_sub(total);
    for slot in order.into_iter().cycle().take(4 * MAX_INFLUENCES) {
        if left == 0 {
            break;
        }
        if scaled[slot] > 0.0 && quantized[slot] < 255 {
            quantized[slot] += 1;
            left -= 1;
        }
    }
    quantized
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::encoding::tests::Random;

    #[test]
    fn weights_always_sum_to_255() {
        assert_eq!(quantize_weights([1.0, 0.0, 0.0, 0.0]), [255, 0, 0, 0]);
        assert_eq!(quantize_weights([0.5, 0.5, 0.0, 0.0]), [128, 127, 0, 0]);
        assert_eq!(quantize_weights([1.0, 1.0, 1.0, 0.0]), [85, 85, 85, 0]);
        assert_eq!(quantize_weights([0.2, 0.8, 0.0, 0.0]), [51, 204, 0, 0]);
        assert_eq!(quantize_weights([0.0; 4]), [255, 0, 0, 0]);
        assert_eq!(quantize_weights([f32::NAN, 1.0, -1.0, 0.0]), [0, 255, 0, 0]);
        assert_eq!(quantize_weights([f32::INFINITY, 1.0, 0.0, 0.0]), [255, 0, 0, 0]);
        let mut random = Random::new(17);
        for _ in 0..10_000 {
            let mut weights: [f32; 4] = std::array::from_fn(|_| random.unit());
            weights[3] *= f32::from(u8::from(random.unit() > 0.5));
            let quantized = quantize_weights(weights);
            assert_eq!(quantized.iter().map(|&q| u32::from(q)).sum::<u32>(), 255, "{weights:?}");
            let sum: f32 = weights.iter().sum();
            for (q, w) in quantized.iter().zip(weights) {
                assert!((f32::from(*q) - w / sum * 255.0).abs() < 1.0, "{weights:?} -> {quantized:?}");
                if w == 0.0 {
                    assert_eq!(*q, 0, "zero weights stay zero");
                }
            }
        }
    }

    #[test]
    fn influences_pack_into_the_narrowest_index() {
        let influences = [
            VertexInfluence { binds: [3, 1, 0, 0], weights: [200, 55, 0, 0] },
            VertexInfluence { binds: [255, 0, 0, 0], weights: [255, 0, 0, 0] },
        ];
        let narrow = Influences::pack(&influences, 256);
        assert!(matches!(&narrow, Influences::U8(v) if v[0].to_bytes() == [3, 1, 0, 0, 200, 55, 0, 0]));
        let wide = Influences::pack(&influences, 300);
        assert!(matches!(&wide, Influences::U16(v) if v[0].to_bytes()[..4] == [3, 0, 1, 0]));
        for packed in [narrow, wide] {
            assert_eq!(packed.len(), 2);
            assert_eq!(
                [packed.get(0), packed.get(1), packed.get(2)],
                [Some(influences[0]), Some(influences[1]), None]
            );
        }
    }
}
