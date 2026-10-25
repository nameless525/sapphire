//! A minimal glTF 2.0 writer for procedural source assets and importer tests.
//!
//! Everything goes into one tightly packed buffer with one buffer view per accessor. That
//! is all the sample generator needs; this is not a general-purpose exporter.

use serde_json::{Value, json};

const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
const UNSIGNED_BYTE: u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;

/// GLB container constants (glTF 2.0 §4.4).
const GLB_MAGIC: &[u8; 4] = b"glTF";
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: &[u8; 4] = b"JSON";
const CHUNK_BIN: &[u8; 4] = b"BIN\0";

/// Vertex and index data for one triangle-list primitive. Empty optional streams are
/// omitted from the output.
#[derive(Clone, Debug, Default)]
pub(crate) struct Geometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// `COLOR_0` as normalized unsigned bytes. glTF defines these as *linear* RGBA.
    pub colors: Vec<[u8; 4]>,
    /// The custom `_OUTLINE_NORMAL` attribute.
    pub outline_normals: Vec<[f32; 3]>,
    /// `JOINTS_n` / `WEIGHTS_n`, one pair per set.
    pub influences: Vec<InfluenceSet>,
    /// Morph targets, in order.
    pub targets: Vec<MorphGeometry>,
    /// Empty means a non-indexed primitive.
    pub indices: Vec<u32>,
}

/// One `JOINTS_n` / `WEIGHTS_n` pair: indices into the skin's joints, and their weights.
#[derive(Clone, Debug, Default)]
pub(crate) struct InfluenceSet {
    pub joints: Vec<[u16; 4]>,
    pub weights: Vec<[f32; 4]>,
}

/// One morph target's displacements. Empty streams are omitted.
#[derive(Clone, Debug, Default)]
pub(crate) struct MorphGeometry {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
}

/// Accumulates a glTF document and its binary buffer.
#[derive(Debug, Default)]
pub(crate) struct GltfWriter {
    bin: Vec<u8>,
    buffer_views: Vec<Value>,
    accessors: Vec<Value>,
    materials: Vec<Value>,
    meshes: Vec<Value>,
    nodes: Vec<Value>,
    skins: Vec<Value>,
    scene_roots: Vec<usize>,
}

impl GltfWriter {
    /// Adds a material (raw glTF JSON) and returns its index.
    pub fn add_material(&mut self, material: Value) -> usize {
        self.materials.push(material);
        self.materials.len() - 1
    }

    /// Adds a mesh with one primitive per `(geometry, material)` pair and returns its index.
    pub fn add_mesh(&mut self, name: &str, primitives: &[(&Geometry, Option<usize>)]) -> usize {
        let primitives: Vec<Value> = primitives
            .iter()
            .map(|&(geometry, material)| {
                let mut primitive = json!({ "attributes": self.add_attributes(geometry) });
                if !geometry.indices.is_empty() {
                    primitive["indices"] = self.add_indices(&geometry.indices).into();
                }
                if !geometry.targets.is_empty() {
                    let targets: Vec<Value> =
                        geometry.targets.iter().map(|target| self.add_target(target)).collect();
                    primitive["targets"] = targets.into();
                }
                if let Some(material) = material {
                    primitive["material"] = material.into();
                }
                primitive
            })
            .collect();
        self.meshes.push(json!({ "name": name, "primitives": primitives }));
        self.meshes.len() - 1
    }

    /// Sets a top-level field of mesh `mesh`, such as `weights` or `extras`.
    #[cfg(test)]
    pub fn set_mesh_field(&mut self, mesh: usize, key: &str, value: Value) {
        self.meshes[mesh][key] = value;
    }

    /// Adds a node (raw glTF JSON: name, mesh, skin, children, TRS) and returns its index.
    pub fn add_node(&mut self, node: Value) -> usize {
        self.nodes.push(node);
        self.nodes.len() - 1
    }

    /// Adds a skin over the `joints` nodes, with one column-major inverse bind matrix per
    /// joint (none means identities), and returns its index.
    #[cfg(test)]
    pub fn add_skin(&mut self, joints: &[usize], inverse_bind_matrices: Option<&[[f32; 16]]>) -> usize {
        let mut skin = json!({ "joints": joints });
        if let Some(matrices) = inverse_bind_matrices {
            skin["inverseBindMatrices"] = self
                .add_accessor(
                    bytemuck::cast_slice(matrices),
                    None,
                    json!({ "componentType": FLOAT, "type": "MAT4", "count": matrices.len() }),
                )
                .into();
        }
        self.skins.push(skin);
        self.skins.len() - 1
    }

    /// Makes `node` a root of the default scene.
    pub fn add_root(&mut self, node: usize) {
        self.scene_roots.push(node);
    }

    /// The binary buffer the document's accessors point into.
    #[cfg(test)]
    pub fn bin(&self) -> &[u8] {
        &self.bin
    }

    /// The glTF JSON document. `buffer_uri` names an external `.bin`; `None` means the
    /// buffer is the GLB `BIN` chunk.
    pub fn document(&self, buffer_uri: Option<&str>) -> Value {
        let mut doc = json!({
            "asset": { "version": "2.0", "generator": "sapphire_pipeline" },
            "scene": 0,
            "scenes": [{ "nodes": self.scene_roots }],
            "nodes": self.nodes,
            "meshes": self.meshes,
            "skins": self.skins,
            "materials": self.materials,
            "accessors": self.accessors,
            "bufferViews": self.buffer_views,
        });
        if !self.bin.is_empty() {
            let mut buffer = json!({ "byteLength": self.bin.len() });
            if let Some(uri) = buffer_uri {
                buffer["uri"] = uri.into();
            }
            doc["buffers"] = json!([buffer]);
        }
        // glTF requires these arrays to be non-empty when present.
        let object = doc.as_object_mut().expect("document is a JSON object");
        object.retain(|_, value| value.as_array().is_none_or(|array| !array.is_empty()));
        doc
    }

    /// The document as a binary glTF (`.glb`) file.
    pub fn to_glb(&self) -> Vec<u8> {
        glb(&self.document(None), &self.bin)
    }

    fn add_attributes(&mut self, geometry: &Geometry) -> Value {
        let mut attributes = json!({ "POSITION": self.add_positions(&geometry.positions) });
        if !geometry.normals.is_empty() {
            attributes["NORMAL"] = self.add_vec3(&geometry.normals).into();
        }
        if !geometry.uvs.is_empty() {
            attributes["TEXCOORD_0"] = self
                .add_accessor(
                    bytemuck::cast_slice(&geometry.uvs),
                    Some(ARRAY_BUFFER),
                    json!({ "componentType": FLOAT, "type": "VEC2", "count": geometry.uvs.len() }),
                )
                .into();
        }
        if !geometry.colors.is_empty() {
            attributes["COLOR_0"] = self
                .add_accessor(
                    bytemuck::cast_slice(&geometry.colors),
                    Some(ARRAY_BUFFER),
                    json!({ "componentType": UNSIGNED_BYTE, "normalized": true, "type": "VEC4",
                            "count": geometry.colors.len() }),
                )
                .into();
        }
        if !geometry.outline_normals.is_empty() {
            attributes["_OUTLINE_NORMAL"] = self.add_vec3(&geometry.outline_normals).into();
        }
        for (set, influences) in geometry.influences.iter().enumerate() {
            // Unsigned bytes whenever every joint fits, as exporters write them.
            let count = influences.joints.len();
            let joints = if influences.joints.iter().flatten().all(|&joint| joint <= u16::from(u8::MAX)) {
                let narrow: Vec<u8> = influences.joints.iter().flatten().map(|&joint| joint as u8).collect();
                let accessor = json!({ "componentType": UNSIGNED_BYTE, "type": "VEC4", "count": count });
                self.add_accessor(&narrow, Some(ARRAY_BUFFER), accessor)
            } else {
                let accessor = json!({ "componentType": UNSIGNED_SHORT, "type": "VEC4", "count": count });
                self.add_accessor(bytemuck::cast_slice(&influences.joints), Some(ARRAY_BUFFER), accessor)
            };
            attributes[format!("JOINTS_{set}")] = joints.into();
            attributes[format!("WEIGHTS_{set}")] = self
                .add_accessor(
                    bytemuck::cast_slice(&influences.weights),
                    Some(ARRAY_BUFFER),
                    json!({ "componentType": FLOAT, "type": "VEC4", "count": influences.weights.len() }),
                )
                .into();
        }
        attributes
    }

    fn add_target(&mut self, target: &MorphGeometry) -> Value {
        let mut attributes = json!({});
        if !target.positions.is_empty() {
            attributes["POSITION"] = self.add_positions(&target.positions).into();
        }
        if !target.normals.is_empty() {
            attributes["NORMAL"] = self.add_vec3(&target.normals).into();
        }
        attributes
    }

    /// A `POSITION` accessor, which glTF requires to carry its bounds.
    fn add_positions(&mut self, positions: &[[f32; 3]]) -> usize {
        let (min, max) = position_bounds(positions);
        self.add_accessor(
            bytemuck::cast_slice(positions),
            Some(ARRAY_BUFFER),
            json!({ "componentType": FLOAT, "type": "VEC3", "count": positions.len(), "min": min, "max": max }),
        )
    }

    fn add_vec3(&mut self, values: &[[f32; 3]]) -> usize {
        self.add_accessor(
            bytemuck::cast_slice(values),
            Some(ARRAY_BUFFER),
            json!({ "componentType": FLOAT, "type": "VEC3", "count": values.len() }),
        )
    }

    fn add_indices(&mut self, indices: &[u32]) -> usize {
        // 16-bit indices whenever they fit, as an exporter would write them.
        let accessor = |component_type| json!({ "componentType": component_type, "type": "SCALAR", "count": indices.len() });
        if indices.iter().all(|&i| i <= u32::from(u16::MAX)) {
            let narrow: Vec<u16> =
                indices.iter().map(|&i| u16::try_from(i).expect("checked above")).collect();
            self.add_accessor(
                bytemuck::cast_slice(&narrow),
                Some(ELEMENT_ARRAY_BUFFER),
                accessor(UNSIGNED_SHORT),
            )
        } else {
            self.add_accessor(
                bytemuck::cast_slice(indices),
                Some(ELEMENT_ARRAY_BUFFER),
                accessor(UNSIGNED_INT),
            )
        }
    }

    /// Adds `bytes` as a buffer view, for vertex data (`target`) or other data such as inverse
    /// bind matrices (`None`), and `accessor` reading it.
    fn add_accessor(&mut self, bytes: &[u8], target: Option<u32>, mut accessor: Value) -> usize {
        // Every component type we write is at most 4 bytes, so 4-byte alignment satisfies
        // glTF's rule that accessor offsets are multiples of the component size.
        pad(&mut self.bin, 0);
        let mut view = json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() });
        if let Some(target) = target {
            view["target"] = target.into();
        }
        self.buffer_views.push(view);
        self.bin.extend_from_slice(bytes);
        accessor["bufferView"] = (self.buffer_views.len() - 1).into();
        self.accessors.push(accessor);
        self.accessors.len() - 1
    }
}

/// Wraps a glTF JSON document and its buffer in a GLB container (glTF 2.0 §4.4): a
/// 12-byte header, a JSON chunk padded with spaces and an optional BIN chunk padded with
/// zeros, each chunk 4-byte aligned.
pub(crate) fn glb(document: &Value, bin: &[u8]) -> Vec<u8> {
    let mut json = serde_json::to_vec(document).expect("serializing a JSON value cannot fail");
    pad(&mut json, b' ');
    let mut bin = bin.to_vec();
    pad(&mut bin, 0);

    let chunk_header_len = 8;
    let mut total = 12 + chunk_header_len + json.len();
    if !bin.is_empty() {
        total += chunk_header_len + bin.len();
    }
    let len32 = |len: usize| u32::try_from(len).expect("GLB larger than 4 GiB").to_le_bytes();

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(GLB_MAGIC);
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&len32(total));
    out.extend_from_slice(&len32(json.len()));
    out.extend_from_slice(CHUNK_JSON);
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        out.extend_from_slice(&len32(bin.len()));
        out.extend_from_slice(CHUNK_BIN);
        out.extend_from_slice(&bin);
    }
    debug_assert_eq!(out.len(), total);
    out
}

fn pad(bytes: &mut Vec<u8>, fill: u8) {
    bytes.resize(bytes.len().next_multiple_of(4), fill);
}

fn position_bounds(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    (min, max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle() -> Geometry {
        Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            indices: vec![0, 1, 2],
            ..Geometry::default()
        }
    }

    #[test]
    fn glb_container_is_aligned_and_parses() {
        let mut writer = GltfWriter::default();
        let mesh = writer.add_mesh("Tri", &[(&triangle(), None)]);
        let node = writer.add_node(json!({ "mesh": mesh }));
        writer.add_root(node);
        let bytes = writer.to_glb();

        assert_eq!(&bytes[..4], GLB_MAGIC);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, bytes.len());
        assert!(bytes.len().is_multiple_of(4));

        let glb = gltf::Glb::from_slice(&bytes).expect("valid GLB container");
        assert!(glb.json.len().is_multiple_of(4));
        assert!(glb.bin.expect("BIN chunk").len().is_multiple_of(4));
        gltf::Gltf::from_slice(&bytes).expect("valid glTF document");
    }

    #[test]
    fn wide_indices_use_u32() {
        let mut writer = GltfWriter::default();
        let index = writer.add_indices(&[0, 70_000, 1]);
        assert_eq!(writer.accessors[index]["componentType"], UNSIGNED_INT);
        let index = writer.add_indices(&[0, 1, 2]);
        assert_eq!(writer.accessors[index]["componentType"], UNSIGNED_SHORT);
    }
}
