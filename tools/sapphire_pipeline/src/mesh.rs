//! The mesh between import and cook: full-precision vertex data in one object space, plus
//! whatever the source authored (skins, morph targets, outline normals).
//!
//! Importers produce an [`ImportedMesh`] with a single LOD; [`bake`](crate::bake) fills in
//! outline normals; [`optimize`](crate::optimize) reorders it for the GPU and adds the LOD
//! chain; [`ImportedMesh::pack`] quantizes it into the cooked
//! [`Mesh`].

use sapphire_assets::mesh::{
    Influences, Joint, Lod, MAX_INFLUENCES, Mesh, MorphDelta, MorphTarget, PositionFormat, Skin, SkinBind,
    ToonMaterialParams, Vertex, VertexInfluence, VertexStreams, quantize_weights,
};

/// An imported mesh. Every per-vertex vector has one entry per vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedMesh {
    pub vertices: Vec<Vertex>,
    /// The source authored this vertex's outline normal (`_OUTLINE_NORMAL`), so the bake keeps
    /// it.
    pub authored_outline: Vec<bool>,
    /// Every LOD's triangle lists.
    pub indices: Vec<u32>,
    /// LOD0 first. Importers write LOD0 alone, with one submesh per source primitive.
    pub lods: Vec<Lod>,
    pub materials: Vec<ToonMaterialParams>,
    pub skin: Option<ImportedSkin>,
    pub morph_targets: Vec<ImportedMorphTarget>,
}

/// A skin before packing: the cooked skeleton and palette, and full-precision weights.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedSkin {
    pub joints: Vec<Joint>,
    pub binds: Vec<SkinBind>,
    /// Per vertex.
    pub weights: Vec<SkinWeights>,
}

/// One vertex's influences: palette indices and normalized weights (summing to 1), heaviest
/// first. Unused slots have weight 0 and index 0.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkinWeights {
    pub binds: [u16; MAX_INFLUENCES],
    pub weights: [f32; MAX_INFLUENCES],
}

/// A morph target before packing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportedMorphTarget {
    pub name: String,
    pub default_weight: f32,
    /// By increasing vertex.
    pub deltas: Vec<ImportedDelta>,
}

/// One vertex's displacement in a morph target.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImportedDelta {
    pub vertex: u32,
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

impl SkinWeights {
    /// Keeps the four heaviest of `influences` (palette index, weight), merging repeats of one
    /// index and ignoring weights that aren't positive, then renormalizes them to sum to 1.
    /// Ties keep the lower index. Returns `None` when no weight is positive.
    pub fn strongest(influences: impl IntoIterator<Item = (u16, f32)>) -> Option<Self> {
        let mut merged: Vec<(u16, f32)> = Vec::with_capacity(MAX_INFLUENCES);
        for (bind, weight) in influences {
            if weight.is_nan() || weight <= 0.0 {
                continue;
            }
            match merged.iter_mut().find(|(existing, _)| *existing == bind) {
                Some((_, total)) => *total += weight,
                None => merged.push((bind, weight)),
            }
        }
        merged.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        merged.truncate(MAX_INFLUENCES);
        let sum: f32 = merged.iter().map(|(_, weight)| weight).sum();
        if !(sum.is_finite() && sum > 0.0) {
            return None;
        }
        let mut result = Self::default();
        for (slot, (bind, weight)) in merged.into_iter().enumerate() {
            result.binds[slot] = bind;
            result.weights[slot] = weight / sum;
        }
        Some(result)
    }

    /// Full weight on palette entry `bind`.
    pub fn rigid(bind: u16) -> Self {
        Self { binds: [bind, 0, 0, 0], weights: [1.0, 0.0, 0.0, 0.0] }
    }
}

impl ImportedMesh {
    /// Packs the mesh into its cooked form.
    pub fn pack(&self, format: PositionFormat) -> Mesh {
        let skin = self.skin.as_ref().map(|skin| {
            let influences: Vec<VertexInfluence> = skin
                .weights
                .iter()
                .map(|w| VertexInfluence { binds: w.binds, weights: quantize_weights(w.weights) })
                .collect();
            Skin {
                joints: skin.joints.clone(),
                binds: skin.binds.clone(),
                influences: Influences::pack(&influences, skin.binds.len()),
            }
        });
        let mut morph_targets = Vec::with_capacity(self.morph_targets.len());
        let mut morph_deltas = Vec::new();
        for target in &self.morph_targets {
            morph_targets.push(MorphTarget {
                name: target.name.clone(),
                default_weight: target.default_weight,
                first_delta: to_u32(morph_deltas.len()),
                delta_count: to_u32(target.deltas.len()),
            });
            morph_deltas
                .extend(target.deltas.iter().map(|d| MorphDelta::new(d.vertex, d.position, d.normal)));
        }
        Mesh {
            streams: VertexStreams::pack(&self.vertices, format),
            indices: self.indices.clone(),
            lods: self.lods.clone(),
            materials: self.materials.clone(),
            skin,
            morph_targets,
            morph_deltas,
        }
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("cooked counts fit u32")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_four_heaviest_influences_renormalized() {
        let weights = SkinWeights::strongest([(5, 0.1), (2, 0.3), (7, 0.05), (2, 0.1), (9, 0.2), (1, 0.25)])
            .expect("positive weights");
        // Joint 2 appears twice (0.4 in all); joint 7 (0.05) is dropped.
        assert_eq!(weights.binds, [2, 1, 9, 5]);
        let kept = [0.4, 0.25, 0.2, 0.1];
        let sum: f32 = kept.iter().sum();
        for (actual, expected) in weights.weights.iter().zip(kept) {
            assert!((actual - expected / sum).abs() < 1e-6, "{weights:?}");
        }
        // Ties keep the lower index; zero, negative and NaN weights are ignored.
        let ties = SkinWeights::strongest([(4, 0.5), (3, 0.5), (8, 0.0), (6, -1.0), (2, f32::NAN)]).unwrap();
        assert_eq!((ties.binds, ties.weights), ([3, 4, 0, 0], [0.5, 0.5, 0.0, 0.0]));
        assert_eq!(SkinWeights::strongest([(1, 0.0)]), None);
        assert_eq!(SkinWeights::rigid(3).binds, [3, 0, 0, 0]);
    }
}
