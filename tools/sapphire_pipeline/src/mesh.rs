//! The mesh between import and cook: full-precision vertex data in one object space, plus
//! whatever the source authored (skins, morph targets, outline normals).
//!
//! Importers produce an [`ImportedMesh`] with a single LOD; [`bake`](crate::bake) fills in
//! outline normals and, for foliage, volume normals and wind data;
//! [`optimize`](crate::optimize) reorders it for the GPU and adds the LOD chain;
//! [`ImportedMesh::pack`] quantizes it into the cooked [`Mesh`] and resolves its material slots.

use std::collections::BTreeMap;

use sapphire_assets::AssetId;
use sapphire_assets::material::{AnimeCharacterMaterial, Material, MaterialDocument, MaterialError};
use sapphire_assets::mesh::{
    Bounds, FloatPosition, FoliageVertex, Influences, Lod, MAX_INFLUENCES, MaterialSlot, Mesh, MorphDelta,
    MorphTarget, PositionFormat, QuantizedPosition, Skin, SkinBind, SlotMaterial, Vertex, VertexInfluence,
    VertexStreams, quantize_weights,
};
use sapphire_assets::skeleton::{Joint, Skeleton};
use sapphire_assets::texture::Swizzle;

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
    /// The material slots submeshes index, as the importer generated them from the source.
    pub materials: Vec<ImportedMaterial>,
    pub skin: Option<ImportedSkin>,
    pub morph_targets: Vec<ImportedMorphTarget>,
    /// Per vertex, the foliage stream's data, once the foliage bake ran (M1 W6,
    /// [`bake::foliage`](crate::bake::foliage)); empty for other meshes.
    pub foliage: Vec<FoliageVertex>,
}

/// A material slot as imported: the source's name for it, and the anime character material
/// generated from the source's description of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportedMaterial {
    /// Unique within the mesh; sidecar bindings name slots by it.
    pub name: String,
    pub generated: AnimeCharacterMaterial,
}

/// A material slot that can't be packed.
#[derive(Debug, thiserror::Error)]
pub enum MaterialSlotError {
    #[error("the sidecar binds material slot {name:?}, but the source's slots are {}", slots.join(", "))]
    UnknownSlot { name: String, slots: Vec<String> },
    #[error(
        "the material generated for slot {slot:?} references textures; textured materials are \
         documents, bound to the slot in the mesh's sidecar"
    )]
    Textured { slot: String },
    #[error("the material generated for slot {slot:?} is invalid")]
    Invalid {
        slot: String,
        #[source]
        source: MaterialError,
    },
}

/// A skin before packing: the joints of the skeleton the source cooks beside the mesh, the cooked
/// palette, and full-precision weights.
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
        Self::strongest_with_scratch(influences, &mut Vec::with_capacity(MAX_INFLUENCES))
    }

    /// Reuses merge storage across a primitive's vertices without changing accumulation order.
    pub(crate) fn strongest_with_scratch(
        influences: impl IntoIterator<Item = (u16, f32)>,
        merged: &mut Vec<(u16, f32)>,
    ) -> Option<Self> {
        merged.clear();
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
        for (slot, &(bind, weight)) in merged.iter().enumerate() {
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
    /// Identifies the imported contact geometry at the mesh's packing precision, before any LOD
    /// is derived. Rendering attributes, optimization order and LOD choice are outside this identity.
    pub(crate) fn authored_geometry(
        &self,
        format: PositionFormat,
        skeleton_id: Option<AssetId>,
        shared: Option<&Skeleton>,
        remap: Option<&[u16]>,
    ) -> [u8; 32] {
        let mut hash = blake3::Hasher::new_derive_key("Sapphire authored mesh geometry v1");
        let bounds = Bounds::enclosing(self.vertices.iter().map(|vertex| vertex.position));
        hash.update(&[format.index() as u8]);
        for axis in bounds.min.into_iter().chain(bounds.max) {
            hash.update(&axis.to_le_bytes());
        }
        hash.update(&to_u32(self.vertices.len()).to_le_bytes());
        for vertex in &self.vertices {
            match format {
                PositionFormat::Unorm16 => {
                    hash.update(&QuantizedPosition::encode(vertex.position, &bounds).to_bytes());
                }
                PositionFormat::Float32 => {
                    hash.update(&FloatPosition::new(vertex.position).to_bytes());
                }
            }
        }
        hash.update(&to_u32(self.indices.len()).to_le_bytes());
        for index in &self.indices {
            hash.update(&index.to_le_bytes());
        }
        hash.update(&[u8::from(self.skin.is_some())]);
        if let Some(skin) = &self.skin {
            hash.update(&[u8::from(skeleton_id.is_some())]);
            if let Some(id) = skeleton_id {
                hash.update(&id.to_bytes());
            }
            let own;
            let skeleton = if let Some(shared) = shared {
                shared
            } else {
                own = Skeleton::new(&skin.joints);
                &own
            };
            hash.update(&to_u32(skeleton.names.len()).to_le_bytes());
            for ((name, parent), rest) in skeleton.names.iter().zip(&skeleton.parents).zip(&skeleton.rest) {
                hash.update(&to_u32(name.len()).to_le_bytes()).update(name.as_bytes());
                hash.update(&parent.to_le_bytes());
                for axis in rest.translation.into_iter().chain(rest.rotation).chain(rest.scale) {
                    hash.update(&axis.to_le_bytes());
                }
            }
            hash.update(&to_u32(skin.binds.len()).to_le_bytes());
            for bind in &skin.binds {
                let joint = remap.map_or(bind.joint, |remap| remap[usize::from(bind.joint)]);
                hash.update(&joint.to_le_bytes());
                for axis in bind.inverse_bind_matrix.into_iter().flatten() {
                    hash.update(&axis.to_le_bytes());
                }
            }
            hash.update(&to_u32(skin.weights.len()).to_le_bytes());
            for weight in &skin.weights {
                for bind in weight.binds {
                    hash.update(&bind.to_le_bytes());
                }
                hash.update(&quantize_weights(weight.weights));
            }
        }
        hash.update(&to_u32(self.morph_targets.len()).to_le_bytes());
        for target in &self.morph_targets {
            hash.update(&to_u32(target.name.len()).to_le_bytes()).update(target.name.as_bytes());
            hash.update(&target.default_weight.to_le_bytes());
            let deltas = target.deltas.iter().filter_map(|delta| {
                let position = MorphDelta::new(delta.vertex, delta.position, [0.0; 3]).position();
                (position != [0.0; 3]).then_some((delta.vertex, position))
            });
            hash.update(&to_u32(deltas.clone().count()).to_le_bytes());
            for (vertex, position) in deltas {
                hash.update(&vertex.to_le_bytes());
                for axis in position {
                    hash.update(&axis.to_le_bytes());
                }
            }
        }
        *hash.finalize().as_bytes()
    }

    /// Packs the mesh into its cooked form. Slots named in `bindings` reference those material
    /// assets; the others embed the material the importer generated.
    pub fn pack(
        &self,
        format: PositionFormat,
        bindings: &BTreeMap<String, AssetId>,
    ) -> Result<Mesh, MaterialSlotError> {
        if let Some(name) = bindings.keys().find(|name| !self.materials.iter().any(|m| &m.name == *name)) {
            let slots = self.materials.iter().map(|m| format!("{:?}", m.name)).collect();
            return Err(MaterialSlotError::UnknownSlot { name: name.clone(), slots });
        }
        let materials =
            self.materials
                .iter()
                .map(|imported| {
                    let material = match bindings.get(&imported.name) {
                        Some(&id) => SlotMaterial::Asset(id),
                        None => {
                            // Generated materials reference no textures, so a mesh never
                            // depends on a texture's cook; textured materials are documents
                            // (`sapphire_assets::material`).
                            let textures = imported.generated.textures();
                            if textures.iter().any(Option::is_some) {
                                return Err(MaterialSlotError::Textured { slot: imported.name.clone() });
                            }
                            let swizzles = vec![Swizzle::IDENTITY; textures.len()];
                            let material = Material::from_document(&imported.generated, &swizzles).map_err(
                                |source| MaterialSlotError::Invalid { slot: imported.name.clone(), source },
                            )?;
                            SlotMaterial::Embedded(material)
                        }
                    };
                    Ok(MaterialSlot { name: imported.name.clone(), material })
                })
                .collect::<Result<_, _>>()?;
        let skin = self.skin.as_ref().map(|skin| {
            let influences: Vec<VertexInfluence> = skin
                .weights
                .iter()
                .map(|w| VertexInfluence { binds: w.binds, weights: quantize_weights(w.weights) })
                .collect();
            Skin {
                // Named once the cook knows the source's ID: see `cook`.
                skeleton: None,
                joint_count: to_u32(skin.joints.len()),
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
        let streams = VertexStreams::pack(&self.vertices, format);
        Ok(Mesh {
            authored_geometry: None,
            streams: if self.foliage.is_empty() { streams } else { streams.with_foliage(&self.foliage) },
            indices: self.indices.clone(),
            lods: self.lods.clone(),
            materials,
            skin,
            morph_targets,
            morph_deltas,
        })
    }
}

fn to_u32(len: usize) -> u32 {
    u32::try_from(len).expect("cooked counts fit u32")
}

#[cfg(test)]
mod tests {
    use sapphire_assets::skeleton::JointTransform;

    use super::*;

    #[test]
    fn authored_geometry_changes_with_contact_inputs() {
        let vertex = |position| Vertex { position, ..Vertex::default() };
        let joint =
            |name: &str, parent| Joint { name: name.to_owned(), parent, rest: JointTransform::IDENTITY };
        let bind = |joint| SkinBind {
            joint,
            inverse_bind_matrix: std::array::from_fn(|column| {
                std::array::from_fn(|row| f32::from(row == column))
            }),
        };
        let mesh = ImportedMesh {
            vertices: vec![vertex([0.0, 0.0, 0.0]), vertex([1.0, 0.0, 0.0]), vertex([0.0, 1.0, 0.0])],
            indices: vec![0, 1, 2],
            skin: Some(ImportedSkin {
                joints: vec![joint("root", None), joint("hand", Some(0))],
                binds: vec![bind(0), bind(1)],
                weights: vec![SkinWeights::rigid(0); 3],
            }),
            morph_targets: vec![ImportedMorphTarget {
                name: "shape".to_owned(),
                deltas: vec![ImportedDelta { vertex: 0, position: [0.1, 0.0, 0.0], normal: [0.0; 3] }],
                ..ImportedMorphTarget::default()
            }],
            ..ImportedMesh::default()
        };
        let identity =
            |mesh: &ImportedMesh| mesh.authored_geometry(PositionFormat::Unorm16, None, None, None);
        let expected = identity(&mesh);
        let skin_id = AssetId::from_bytes([1; 16]);
        let mut shared = Skeleton::new(&mesh.skin.as_ref().unwrap().joints);
        let shared_identity = |id, skeleton: &Skeleton, remap: &[u16]| {
            mesh.authored_geometry(PositionFormat::Unorm16, Some(id), Some(skeleton), Some(remap))
        };
        let resolved = mesh.authored_geometry(PositionFormat::Unorm16, Some(skin_id), None, None);
        assert_eq!(resolved, shared_identity(skin_id, &shared, &[0, 1]));
        assert_ne!(resolved, shared_identity(skin_id, &shared, &[1, 0]));
        assert_ne!(resolved, shared_identity(AssetId::from_bytes([2; 16]), &shared, &[0, 1]));
        shared.rest[1].translation[0] = 0.1;
        assert_ne!(resolved, shared_identity(skin_id, &shared, &[0, 1]));
        let mutations: [fn(&mut ImportedMesh); 6] = [
            |mesh| mesh.vertices[0].position[1] = 0.1,
            |mesh| mesh.indices.swap(1, 2),
            |mesh| mesh.skin.as_mut().unwrap().joints[1].rest.translation[1] = 0.1,
            |mesh| mesh.skin.as_mut().unwrap().binds[1].inverse_bind_matrix[3][0] = 0.1,
            |mesh| mesh.skin.as_mut().unwrap().weights[1] = SkinWeights::rigid(1),
            |mesh| mesh.morph_targets[0].deltas[0].position[1] = 0.1,
        ];
        for mutate in mutations {
            let mut changed = mesh.clone();
            mutate(&mut changed);
            assert_ne!(expected, identity(&changed));
        }
        let mut shading = mesh;
        shading.morph_targets[0].deltas[0].normal[0] = 0.1;
        shading.morph_targets[0].deltas.push(ImportedDelta {
            vertex: 1,
            position: [0.0; 3],
            normal: [0.1, 0.0, 0.0],
        });
        assert_eq!(expected, identity(&shading));
    }

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
