//! Malformed files. Every row of these tables is a crafted document that must come back as an
//! error saying what's wrong, never a panic: a creator adding one bad model gets an error, not a
//! dead cook. Files are built in code, with their buffer in a `data:` URI.

use std::panic::{self, AssertUnwindSafe};

use serde_json::{Value, json};

use super::*;
use crate::cook::{Cook, CookError, cook_mesh};
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry};
use crate::meta::MeshSettings;
use crate::test_util::TempDir;

const BYTE: u64 = 5120;
const UNSIGNED_BYTE: u64 = 5121;
const SHORT: u64 = 5122;
const UNSIGNED_SHORT: u64 = 5123;
const UNSIGNED_INT: u64 = 5125;
const FLOAT: u64 = 5126;

/// A count, offset or length no file could really hold, where the readers' arithmetic overflows.
const HUGE: u64 = u64::MAX;

/// A crafted file: its JSON, and the buffer that goes into its `data:` URI.
struct File {
    doc: Value,
    bin: Vec<u8>,
}

impl File {
    /// One skinned triangle with a morph target and every accessor the importer reads, plus a
    /// static triangle beside it. It imports cleanly, so a row's edit is what breaks it.
    fn figure() -> Self {
        let writer = Self::figure_writer();
        Self { doc: writer.document(None), bin: writer.bin().to_vec() }
    }

    fn figure_writer() -> GltfWriter {
        let triangle = || vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]];
        let skinned = Geometry {
            positions: triangle(),
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.25, 0.5]; 3],
            colors: vec![[255; 4]; 3],
            outline_normals: vec![[0.0, 0.0, 1.0]; 3],
            material_ids: vec![1.0; 3],
            influences: vec![InfluenceSet {
                joints: vec![[0, 0, 0, 0]; 3],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
            }],
            targets: vec![MorphGeometry {
                positions: vec![[0.0, 0.1, 0.0]; 3],
                normals: vec![[0.0, 0.0, 0.1]; 3],
            }],
            indices: vec![0, 1, 2],
        };
        let plain = Geometry { positions: triangle(), ..Geometry::default() };
        let mut writer = GltfWriter::default();
        let body = writer.add_mesh("Body", &[(&skinned, None)]);
        let prop = writer.add_mesh("Prop", &[(&plain, None)]);
        let joint = writer.add_node(json!({ "name": "Joint" }));
        writer.add_skin(&[joint], Some(&[Mat4::IDENTITY.to_cols_array()]));
        let body = writer.add_node(json!({ "name": "BodyNode", "mesh": body, "skin": 0 }));
        let prop = writer.add_node(json!({ "name": "PropNode", "mesh": prop }));
        for root in [joint, body, prop] {
            writer.add_root(root);
        }
        writer
    }

    /// A file of only `nodes`, with the scene's `roots`, and a triangle as mesh 0 for them to name.
    fn nodes(nodes: Value, roots: Value) -> Self {
        let geometry = Geometry {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            ..Geometry::default()
        };
        let mut writer = GltfWriter::default();
        writer.add_mesh("Mesh", &[(&geometry, None)]);
        let mut doc = writer.document(None);
        doc["nodes"] = nodes;
        doc["scenes"] = json!([{ "nodes": roots }]);
        Self { doc, bin: writer.bin().to_vec() }
    }

    fn primitive(&mut self) -> &mut Value {
        &mut self.doc["meshes"][0]["primitives"][0]
    }

    fn index_of(value: &Value) -> usize {
        value.as_u64().expect("an accessor index") as usize
    }

    fn accessor(&mut self, index: usize) -> &mut Value {
        &mut self.doc["accessors"][index]
    }

    /// The accessor of attribute `name` of the skinned primitive.
    fn attribute(&mut self, name: &str) -> &mut Value {
        let index = Self::index_of(&self.primitive()["attributes"][name]);
        self.accessor(index)
    }

    fn indices(&mut self) -> &mut Value {
        let index = Self::index_of(&self.primitive()["indices"]);
        self.accessor(index)
    }

    /// The accessor of `name` in the skinned primitive's morph target.
    fn target(&mut self, name: &str) -> &mut Value {
        let index = Self::index_of(&self.primitive()["targets"][0][name]);
        self.accessor(index)
    }

    fn matrices(&mut self) -> &mut Value {
        let index = Self::index_of(&self.doc["skins"][0]["inverseBindMatrices"]);
        self.accessor(index)
    }

    /// The buffer view of `accessor`.
    fn view_of(&mut self, accessor: fn(&mut Self) -> &mut Value) -> &mut Value {
        let index = Self::index_of(&accessor(self)["bufferView"]);
        &mut self.doc["bufferViews"][index]
    }

    /// Gives `accessor` a sparse block over `count` elements, its indices and values in the
    /// document's first buffer view.
    fn sparse(&mut self, accessor: fn(&mut Self) -> &mut Value, count: u64) {
        let sparse = json!({
            "count": count,
            "indices": { "bufferView": 0, "componentType": UNSIGNED_SHORT },
            "values": { "bufferView": 0 },
        });
        accessor(self)["sparse"] = sparse;
    }

    /// Takes `accessor`'s buffer view away and makes it `count` zeros with one sparse value:
    /// what a small file can claim.
    fn sparse_only(&mut self, accessor: fn(&mut Self) -> &mut Value, count: u64) {
        let target = accessor(self);
        let view = target.as_object_mut().expect("an accessor").remove("bufferView");
        target["count"] = count.into();
        target["sparse"] = json!({
            "count": 1,
            "indices": { "bufferView": view, "componentType": UNSIGNED_SHORT },
            "values": { "bufferView": view },
        });
    }

    /// The file as a `.gltf`, its buffer in a `data:` URI.
    fn bytes(&self) -> Vec<u8> {
        let mut doc = self.doc.clone();
        doc["buffers"][0]["uri"] =
            format!("data:application/octet-stream;base64,{}", base64(&self.bin)).into();
        doc["buffers"][0]["byteLength"] = self.bin.len().into();
        serde_json::to_vec(&doc).expect("a JSON value serializes")
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word =
            chunk.iter().fold(0u32, |word, &byte| (word << 8) | u32::from(byte)) << (8 * (3 - chunk.len()));
        for k in 0..=chunk.len() {
            out.push(char::from(ALPHABET[(word >> (18 - 6 * k)) as usize & 63]));
        }
    }
    out
}

/// Writes `value` into the file's buffer, as component `component` of vertex `vertex`'s position
/// in the skinned primitive.
fn poison_position(file: &mut File, vertex: usize, component: usize, value: f32) {
    let accessor = File::index_of(&file.primitive()["attributes"]["POSITION"]);
    let view = File::index_of(&file.doc["accessors"][accessor]["bufferView"]);
    let offset = file.doc["bufferViews"][view]["byteOffset"].as_u64().unwrap_or(0) as usize;
    let at = offset + 12 * vertex + 4 * component;
    file.bin[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

struct Case {
    name: &'static str,
    craft: fn(&mut File),
    /// What the error must say, so it helps a creator find the problem.
    says: &'static str,
}

const fn case(name: &'static str, craft: fn(&mut File), says: &'static str) -> Case {
    Case { name, craft, says }
}

/// The message of a panic.
fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic".to_owned())
}

/// Runs each case on a fresh figure, and fails listing every one that didn't give a clean error.
fn assert_all_fail(cases: &[Case]) {
    let mut wrong = Vec::new();
    for Case { name, craft, says } in cases {
        let mut file = File::figure();
        craft(&mut file);
        let bytes = file.bytes();
        match panic::catch_unwind(AssertUnwindSafe(|| import_slice(&bytes, None))) {
            Err(payload) => wrong.push(format!("{name}: panicked: {}", panic_text(&*payload))),
            Ok(Ok(mesh)) => wrong.push(format!("{name}: imported {} vertices", mesh.vertices.len())),
            Ok(Err(error)) => {
                let text = error.to_string();
                if !text.contains(says) {
                    wrong.push(format!("{name}: said {text:?}, which doesn't mention {says:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {} cases:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

#[test]
fn the_figure_imports_cleanly() {
    let mesh = import_slice(&File::figure().bytes(), None).expect("the figure imports");
    assert_eq!(mesh.vertices.len(), 6);
    assert_eq!(mesh.morph_targets.len(), 1);
}

/// Accessors whose types the `gltf` readers don't expect (AS-4).
#[test]
fn off_spec_accessor_types_are_errors() {
    fn u8_positions(file: &mut File) {
        file.attribute("POSITION")["componentType"] = UNSIGNED_BYTE.into();
    }
    assert_all_fail(&[
        case("indices_float", |f| f.indices()["componentType"] = FLOAT.into(), "indices"),
        case("indices_i8", |f| f.indices()["componentType"] = BYTE.into(), "indices"),
        case("indices_i16", |f| f.indices()["componentType"] = SHORT.into(), "indices"),
        case("indices_vec3", |f| f.indices()["type"] = "VEC3".into(), "indices"),
        case("indices_normalized", |f| f.indices()["normalized"] = true.into(), "indices"),
        case("position_u8", u8_positions, "POSITION"),
        case("position_normalized", |f| f.attribute("POSITION")["normalized"] = true.into(), "POSITION"),
        case(
            "position_quantized_in_extensions_used",
            |f| {
                f.doc["extensionsUsed"] = json!(["KHR_mesh_quantization"]);
                u8_positions(f);
            },
            "POSITION",
        ),
        case(
            "position_quantized_in_extensions_required",
            |f| {
                f.doc["extensionsRequired"] = json!(["KHR_mesh_quantization"]);
                u8_positions(f);
            },
            "not a valid glTF",
        ),
        case("normal_i8", |f| f.attribute("NORMAL")["componentType"] = BYTE.into(), "NORMAL"),
        case("normal_scalar", |f| f.attribute("NORMAL")["type"] = "SCALAR".into(), "NORMAL"),
        case("texcoord_vec3", |f| f.attribute("TEXCOORD_0")["type"] = "VEC3".into(), "TEXCOORD_0"),
        case("texcoord_i16", |f| f.attribute("TEXCOORD_0")["componentType"] = SHORT.into(), "TEXCOORD_0"),
        case(
            "texcoord_u16_not_normalized",
            |f| f.attribute("TEXCOORD_0")["componentType"] = UNSIGNED_SHORT.into(),
            "TEXCOORD_0",
        ),
        case("color_scalar", |f| f.attribute("COLOR_0")["type"] = "SCALAR".into(), "COLOR_0"),
        case("color_i8", |f| f.attribute("COLOR_0")["componentType"] = BYTE.into(), "COLOR_0"),
        case("joints_u32", |f| f.attribute("JOINTS_0")["componentType"] = UNSIGNED_INT.into(), "JOINTS_0"),
        case("joints_float", |f| f.attribute("JOINTS_0")["componentType"] = FLOAT.into(), "JOINTS_0"),
        case("weights_i16", |f| f.attribute("WEIGHTS_0")["componentType"] = SHORT.into(), "WEIGHTS_0"),
        case(
            "outline_normal_u8",
            |f| f.attribute("_OUTLINE_NORMAL")["componentType"] = UNSIGNED_BYTE.into(),
            "_OUTLINE_NORMAL",
        ),
        case("material_id_vec2", |f| f.attribute("_MATERIAL_ID")["type"] = "VEC2".into(), "_MATERIAL_ID"),
        case(
            "morph_position_i8",
            |f| f.target("POSITION")["componentType"] = BYTE.into(),
            "morph target 0 POSITION",
        ),
        case(
            "morph_position_i16",
            |f| f.target("POSITION")["componentType"] = SHORT.into(),
            "morph target 0 POSITION",
        ),
        case(
            "morph_position_scalar_short",
            |f| {
                f.target("POSITION")["type"] = "SCALAR".into();
                f.target("POSITION")["componentType"] = SHORT.into();
            },
            "morph target 0 POSITION",
        ),
        case(
            "morph_normal_scalar_short",
            |f| {
                f.target("NORMAL")["type"] = "SCALAR".into();
                f.target("NORMAL")["componentType"] = SHORT.into();
            },
            "morph target 0 NORMAL",
        ),
        case("inverse_binds_scalar", |f| f.matrices()["type"] = "SCALAR".into(), "skin 0"),
        case("inverse_binds_mat4_u8", |f| f.matrices()["componentType"] = UNSIGNED_BYTE.into(), "skin 0"),
        case("inverse_binds_mat3", |f| f.matrices()["type"] = "MAT3".into(), "skin 0"),
    ]);
}

/// Accessors and views whose numbers don't fit their buffers (AS-4).
#[test]
fn accessors_that_do_not_fit_are_errors() {
    assert_all_fail(&[
        case(
            "position_stride_too_small",
            |f| f.view_of(|f| f.attribute("POSITION"))["byteStride"] = 4.into(),
            "POSITION",
        ),
        case(
            "normal_stride_too_small",
            |f| f.view_of(|f| f.attribute("NORMAL"))["byteStride"] = 8.into(),
            "NORMAL",
        ),
        case(
            "morph_stride_too_small",
            |f| f.view_of(|f| f.target("POSITION"))["byteStride"] = 4.into(),
            "morph target 0 POSITION",
        ),
        case(
            "inverse_binds_stride_too_small",
            |f| f.view_of(File::matrices)["byteStride"] = 16.into(),
            "skin 0",
        ),
        case("position_no_elements", |f| f.attribute("POSITION")["count"] = 0.into(), "POSITION"),
        case("indices_no_elements", |f| f.indices()["count"] = 0.into(), "indices"),
        case("normal_no_elements", |f| f.attribute("NORMAL")["count"] = 0.into(), "NORMAL"),
        case("morph_no_elements", |f| f.target("POSITION")["count"] = 0.into(), "morph target 0 POSITION"),
        case("inverse_binds_no_elements", |f| f.matrices()["count"] = 0.into(), "skin 0"),
        case("position_count_huge", |f| f.attribute("POSITION")["count"] = HUGE.into(), "POSITION"),
        case("indices_count_huge", |f| f.indices()["count"] = HUGE.into(), "indices"),
        case("normal_count_huge", |f| f.attribute("NORMAL")["count"] = HUGE.into(), "NORMAL"),
        case("joints_count_huge", |f| f.attribute("JOINTS_0")["count"] = HUGE.into(), "JOINTS_0"),
        case("morph_count_huge", |f| f.target("NORMAL")["count"] = HUGE.into(), "morph target 0 NORMAL"),
        case("inverse_binds_count_huge", |f| f.matrices()["count"] = HUGE.into(), "skin 0"),
        case("position_offset_huge", |f| f.attribute("POSITION")["byteOffset"] = HUGE.into(), "POSITION"),
        case("indices_offset_huge", |f| f.indices()["byteOffset"] = HUGE.into(), "indices"),
        case("inverse_binds_offset_huge", |f| f.matrices()["byteOffset"] = HUGE.into(), "skin 0"),
        case("position_past_its_view", |f| f.attribute("POSITION")["byteOffset"] = 4.into(), "POSITION"),
        case(
            "view_offset_huge",
            |f| f.view_of(|f| f.attribute("POSITION"))["byteOffset"] = HUGE.into(),
            "POSITION",
        ),
        case(
            "view_length_huge",
            |f| f.view_of(|f| f.attribute("POSITION"))["byteLength"] = HUGE.into(),
            "POSITION",
        ),
        case(
            "view_past_its_buffer",
            |f| f.view_of(|f| f.attribute("POSITION"))["byteLength"] = 1_000_000.into(),
            "POSITION",
        ),
        case(
            "normal_too_few",
            |f| f.attribute("NORMAL")["count"] = 2.into(),
            "NORMAL has 2 elements but POSITION has 3",
        ),
        case(
            "morph_too_few",
            |f| f.target("POSITION")["count"] = 2.into(),
            "morph target 0 POSITION has 2 elements but POSITION has 3",
        ),
    ]);
}

/// Sparse accessors: the readers trust their counts and views too (AS-4, AS-5).
#[test]
fn broken_sparse_accessors_are_errors() {
    fn set_sparse(file: &mut File, value: Value) {
        file.attribute("POSITION")["sparse"] = value;
    }
    assert_all_fail(&[
        case("sparse_no_elements", |f| f.sparse(|f| f.attribute("POSITION"), 0), "POSITION"),
        case("sparse_more_than_its_accessor", |f| f.sparse(|f| f.attribute("POSITION"), 4), "POSITION"),
        case("sparse_count_huge", |f| f.sparse(|f| f.attribute("POSITION"), HUGE), "POSITION"),
        case("sparse_duplicate_indices", |f| f.sparse(|f| f.attribute("POSITION"), 2), "sparse index"),
        case(
            "sparse_out_of_range_index",
            |f| {
                f.sparse(|f| f.attribute("POSITION"), 1);
                f.bin[0..2].copy_from_slice(&3u16.to_le_bytes());
            },
            "sparse index",
        ),
        case(
            "sparse_descending_indices",
            |f| {
                f.sparse(|f| f.attribute("POSITION"), 2);
                f.bin[0..2].copy_from_slice(&1u16.to_le_bytes());
                f.bin[2..4].copy_from_slice(&0u16.to_le_bytes());
            },
            "sparse index",
        ),
        case(
            "sparse_indices_float",
            |f| {
                f.sparse(|f| f.attribute("POSITION"), 1);
                f.attribute("POSITION")["sparse"]["indices"]["componentType"] = FLOAT.into();
            },
            "not a valid glTF",
        ),
        case(
            "sparse_indices_offset_huge",
            |f| {
                f.sparse(|f| f.attribute("POSITION"), 1);
                f.attribute("POSITION")["sparse"]["indices"]["byteOffset"] = HUGE.into();
            },
            "POSITION",
        ),
        case(
            "sparse_values_offset_huge",
            |f| {
                f.sparse(|f| f.attribute("POSITION"), 1);
                f.attribute("POSITION")["sparse"]["values"]["byteOffset"] = HUGE.into();
            },
            "POSITION",
        ),
        case(
            "sparse_values_past_their_view",
            |f| {
                set_sparse(
                    f,
                    json!({
                        "count": 2,
                        "indices": { "bufferView": 0, "componentType": UNSIGNED_SHORT },
                        "values": { "bufferView": 0, "byteOffset": 12 },
                    }),
                );
                f.doc["bufferViews"][0]["byteLength"] = 12.into();
            },
            "POSITION",
        ),
        case(
            "sparse_values_stride_too_small",
            |f| {
                f.sparse(|f| f.attribute("POSITION"), 1);
                f.doc["bufferViews"][0]["byteStride"] = 4.into();
            },
            "POSITION",
        ),
        case(
            "sparse_only_position_claims_all",
            |f| f.sparse_only(|f| f.attribute("POSITION"), 1 << 62),
            "vertices",
        ),
        case("sparse_only_indices_claims_all", |f| f.sparse_only(File::indices, 1 << 62), "indices"),
        case(
            "sparse_only_normal_claims_all",
            |f| f.sparse_only(|f| f.attribute("NORMAL"), 1 << 62),
            "NORMAL",
        ),
        case(
            "sparse_only_morph_claims_all",
            |f| f.sparse_only(|f| f.target("POSITION"), 1 << 62),
            "morph target 0 POSITION",
        ),
        case("sparse_only_inverse_binds_claims_all", |f| f.sparse_only(File::matrices, 1 << 62), "skin 0"),
    ]);
}

/// A skin without joints (AS-6): beside a static mesh it panicked, alone it failed late.
#[test]
fn a_skin_without_joints_is_an_error() {
    assert_all_fail(&[
        case(
            "skin_no_joints_static_beside",
            |f| {
                f.doc["skins"][0] = json!({ "joints": [] });
                f.primitive()["attributes"] = json!({
                    "POSITION": f.doc["meshes"][1]["primitives"][0]["attributes"]["POSITION"].clone(),
                });
                f.primitive().as_object_mut().expect("a primitive").remove("targets");
            },
            "skin 0",
        ),
        case(
            "skin_no_joints_alone",
            |f| {
                f.doc["skins"][0] = json!({ "joints": [] });
                f.doc["scenes"][0]["nodes"] = json!([1]);
            },
            "skin 0",
        ),
    ]);
}

/// Positions that aren't numbers (AS-7): one NaN used to land on the mesh's bounds corner.
#[test]
fn non_finite_positions_are_errors() {
    assert_all_fail(&[
        case("position_one_nan", |f| poison_position(f, 1, 0, f32::NAN), "vertex 1"),
        case("position_first_nan", |f| poison_position(f, 0, 2, f32::NAN), "vertex 0"),
        case("position_infinite", |f| poison_position(f, 2, 1, f32::INFINITY), "vertex 2"),
        case(
            "position_all_nan",
            |f| {
                for vertex in 0..3 {
                    for component in 0..3 {
                        poison_position(f, vertex, component, f32::NAN);
                    }
                }
            },
            "vertex 0",
        ),
    ]);
}

#[test]
fn the_error_names_the_mesh_and_vertex_of_a_non_finite_position() {
    let mut file = File::figure();
    poison_position(&mut file, 1, 0, f32::NAN);
    let error = import_slice(&file.bytes(), None).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("mesh \"Body\" primitive 0") && text.contains("vertex 1"), "{text}");
}

/// glTF's nodes form strict trees (AS-5): a node reached twice instanced everything under it
/// twice, so each level of a small file doubled the mesh.
#[test]
fn a_node_reached_twice_is_an_error() {
    fn check(nodes: Value, roots: Value) {
        let error = import_slice(&File::nodes(nodes, roots).bytes(), None).expect_err("not a tree");
        assert!(error.to_string().contains("reached twice"), "{error}");
    }
    // A diamond: 0 → 1, 2; 1 → 3; 2 → 3.
    check(
        json!([{ "children": [1, 2] }, { "children": [3] }, { "children": [3] }, { "mesh": 0 }]),
        json!([0]),
    );
    // Named twice by one parent.
    check(json!([{ "children": [1, 1] }, { "mesh": 0 }]), json!([0]));
    // A root that is also a child, and one listed twice.
    check(json!([{ "children": [1] }, { "mesh": 0 }]), json!([0, 1]));
    check(json!([{ "mesh": 0 }]), json!([0, 0]));
    // Cycles: to itself, and through a child.
    check(json!([{ "mesh": 0, "children": [0] }]), json!([0]));
    check(json!([{ "children": [1] }, { "children": [0] }]), json!([0]));
}

#[test]
fn a_doubling_graph_is_an_error() {
    // Each node names the next twice, so 16 levels were 3 × 2^16 vertices from under 2 KB.
    let levels = 16;
    let mut nodes: Vec<Value> = (0..levels).map(|n| json!({ "children": [n + 1, n + 1] })).collect();
    nodes.push(json!({ "mesh": 0 }));
    let file = File::nodes(nodes.into(), json!([0]));
    let error = import_slice(&file.bytes(), None).expect_err("not a tree");
    assert!(error.to_string().contains("reached twice"), "{error}");
}

/// A chain is a legal tree, however deep: it must import, not overflow the stack.
#[test]
fn a_deep_chain_imports_without_overflowing_the_stack() {
    let depth = 200_000;
    let mut nodes: Vec<Value> = (1..depth).map(|n| json!({ "children": [n] })).collect();
    nodes.push(json!({ "mesh": 0 }));
    let file = File::nodes(nodes.into(), json!([0]));
    let mesh = import_slice(&file.bytes(), None).expect("a chain is a tree");
    assert_eq!(mesh.vertices.len(), 3);
}

/// References to things that aren't there. The crate's validation finds them, but for `POSITION`
/// it would panic first, indexing the accessors with it to read the bounds.
#[test]
fn references_to_nothing_are_errors() {
    const INVALID: &str = "not a valid glTF";
    assert_all_fail(&[
        case("position_accessor", |f| f.primitive()["attributes"]["POSITION"] = 99.into(), INVALID),
        case("normal_accessor", |f| f.primitive()["attributes"]["NORMAL"] = 99.into(), INVALID),
        case("indices_accessor", |f| f.primitive()["indices"] = 99.into(), INVALID),
        case("morph_accessor", |f| f.primitive()["targets"][0]["POSITION"] = 99.into(), INVALID),
        case("inverse_binds_accessor", |f| f.doc["skins"][0]["inverseBindMatrices"] = 99.into(), INVALID),
        case("accessor_view", |f| f.attribute("POSITION")["bufferView"] = 99.into(), INVALID),
        case("view_buffer", |f| f.doc["bufferViews"][0]["buffer"] = 99.into(), INVALID),
        case("material", |f| f.primitive()["material"] = 99.into(), INVALID),
        case("node_mesh", |f| f.doc["nodes"][1]["mesh"] = 99.into(), INVALID),
        case("node_skin", |f| f.doc["nodes"][1]["skin"] = 99.into(), INVALID),
        case("node_child", |f| f.doc["nodes"][0]["children"] = json!([99]), INVALID),
        case("skin_joint", |f| f.doc["skins"][0]["joints"] = json!([99]), INVALID),
        case("scene_root", |f| f.doc["scenes"][0]["nodes"] = json!([99]), INVALID),
        case("scene", |f| f.doc["scene"] = 99.into(), INVALID),
    ]);
}

/// What a sparse accessor can claim once the arithmetic no longer overflows: enough to fill
/// memory, from a file of about a kilobyte.
#[test]
fn claims_past_the_budget_fail_before_anything_is_read() {
    assert_all_fail(&[
        case("position_40_bits", |f| f.sparse_only(|f| f.attribute("POSITION"), 1 << 40), "vertices"),
        case("indices_40_bits", |f| f.sparse_only(File::indices, 1 << 40), "indices"),
        case("normal_40_bits", |f| f.sparse_only(|f| f.attribute("NORMAL"), 1 << 40), "NORMAL has"),
        case("joints_40_bits", |f| f.sparse_only(|f| f.attribute("JOINTS_0"), 1 << 40), "JOINTS_0 has"),
        case(
            "morph_40_bits",
            |f| f.sparse_only(|f| f.target("NORMAL"), 1 << 40),
            "morph target 0 NORMAL has",
        ),
        case("inverse_binds_40_bits", |f| f.sparse_only(File::matrices, 1 << 40), "inverse bind matrices"),
    ]);
}

/// Imports `file` within `budget`, as [`import_slice`] does within [`SOURCE_BUDGET`].
fn import_within(file: &File, budget: MeshBudget) -> Result<ImportedMesh, GltfImportError> {
    let Gltf { document, blob } = parse(&file.bytes()).map_err(GltfImportError::Parse)?;
    let buffers = load_buffers(&document, None, blob)?;
    import_scene_within(&document, &buffers, &[], budget).map(|scene| scene.mesh)
}

/// The figure's budget, exactly what it holds: 3 skinned vertices and 3 static, 6 indices, 3 morph
/// deltas and one set of 3 vertices' joints.
const FIGURE_BUDGET: MeshBudget = MeshBudget { vertices: 6, indices: 6, morph_deltas: 3, influence_sets: 3 };

fn assert_over(error: &GltfImportError, expected: &str, expected_found: usize, expected_budget: usize) {
    let GltfImportError::OverBudget { what, found, budget, .. } = error else { panic!("{error:?}") };
    assert_eq!((*what, *found, *budget), (expected, expected_found, expected_budget), "{error}");
}

#[test]
fn a_file_at_its_budget_imports_and_one_over_fails() {
    let file = File::figure();
    let mesh = import_within(&file, FIGURE_BUDGET).expect("exactly at the budget");
    assert_eq!((mesh.vertices.len(), mesh.indices.len()), (6, 6));
    let over = |change: fn(&mut MeshBudget)| {
        let mut budget = FIGURE_BUDGET;
        change(&mut budget);
        import_within(&file, budget).unwrap_err()
    };
    // The static triangle takes the scene past 5 vertices.
    assert_over(&over(|budget| budget.vertices = 5), "vertices", 6, 5);
    assert_over(&over(|budget| budget.indices = 5), "indices", 6, 5);
    assert_over(&over(|budget| budget.morph_deltas = 2), "morph deltas", 3, 2);
    assert_over(&over(|budget| budget.influence_sets = 2), "vertices × JOINTS sets", 3, 2);
    // One primitive alone over the budget fails before it is read.
    assert_over(&over(|budget| budget.vertices = 2), "vertices", 3, 2);
}

/// Each instance of a mesh brings its vertices, so the budget holds for the scene.
#[test]
fn the_budget_counts_every_instance() {
    let nodes = json!([{ "children": [1, 2, 3] }, { "mesh": 0 }, { "mesh": 0 }, { "mesh": 0 }]);
    let file = File::nodes(nodes, json!([0]));
    let budget = MeshBudget { vertices: 8, ..SOURCE_BUDGET };
    assert_over(&import_within(&file, budget).unwrap_err(), "vertices", 9, 8);
    let budget = MeshBudget { indices: 8, ..SOURCE_BUDGET };
    assert_over(&import_within(&file, budget).unwrap_err(), "indices", 9, 8);
    let budget = MeshBudget { vertices: 9, indices: 9, ..SOURCE_BUDGET };
    assert_eq!(import_within(&file, budget).expect("at the budget").vertices.len(), 9);
}

/// `JOINTS_n` sets may all name one accessor, multiplying what a file holds in memory.
#[test]
fn the_budget_counts_every_influence_set() {
    let mut file = File::figure();
    let attributes = &mut file.primitive()["attributes"];
    let (joints, weights) = (attributes["JOINTS_0"].clone(), attributes["WEIGHTS_0"].clone());
    for set in 1..4 {
        attributes[format!("JOINTS_{set}")] = joints.clone();
        attributes[format!("WEIGHTS_{set}")] = weights.clone();
    }
    let budget = MeshBudget { influence_sets: 11, ..SOURCE_BUDGET };
    assert_over(&import_within(&file, budget).unwrap_err(), "vertices × JOINTS sets", 12, 11);
    import_within(&file, MeshBudget { influence_sets: 12, ..SOURCE_BUDGET }).expect("at the budget");
}

/// Every morph target's deltas count.
#[test]
fn the_budget_counts_every_morph_delta() {
    let mut file = File::figure();
    let target = file.primitive()["targets"][0].clone();
    file.primitive()["targets"] = json!([target, target]);
    let budget = MeshBudget { morph_deltas: 5, ..SOURCE_BUDGET };
    assert_over(&import_within(&file, budget).unwrap_err(), "morph deltas", 6, 5);
    import_within(&file, MeshBudget { morph_deltas: 6, ..SOURCE_BUDGET }).expect("at the budget");
}

/// A sparse accessor within its limits still reads, with no view of its own.
#[test]
fn a_sparse_only_accessor_within_its_limits_reads() {
    let mut file = File::figure();
    file.sparse_only(|f| f.target("POSITION"), 3);
    import_slice(&file.bytes(), None).expect("a sparse-only displacement");
}

/// Through the cook, on both cooks, the error names the file, as every failure must.
#[test]
fn a_cook_names_the_file_and_the_problem() {
    let dir = TempDir::new("malformed_cook");
    let mut nan = File::figure();
    poison_position(&mut nan, 1, 0, f32::NAN);
    let mut float_indices = File::figure();
    float_indices.indices()["componentType"] = FLOAT.into();
    let twice = File::nodes(json!([{ "children": [1, 1] }, { "mesh": 0 }]), json!([0]));
    let mut no_joints = File::figure();
    no_joints.doc["skins"][0] = json!({ "joints": [] });
    for (name, file, says) in [
        ("nan.gltf", nan, "mesh \"Body\" primitive 0: vertex 1"),
        ("float_indices.gltf", float_indices, "the indices accessor"),
        ("twice.gltf", twice, "node 1 is reached twice"),
        ("no_joints.gltf", no_joints, "skin 0 has no joints"),
    ] {
        let path = dir.path().join(name);
        std::fs::write(&path, file.bytes()).expect("write the file");
        for cook in [Cook::Desktop, Cook::Mobile] {
            let error = cook_mesh(&path, &MeshSettings::default(), cook).expect_err(name);
            assert!(error.to_string().contains(name), "{cook:?}: {error}");
            let CookError::Import { source, .. } = &error else { panic!("{cook:?}: {error:?}") };
            assert!(source.to_string().contains(says), "{cook:?}: {source}");
        }
    }
}

/// The channels of [`File::animated`].
const TRANSLATION: usize = 0;
const ROTATION: usize = 1;
const SCALE: usize = 2;
const WEIGHTS: usize = 3;

impl File {
    /// The figure with an animation of two keys in each kind of channel, in the order of the
    /// constants above: translation, rotation and scale of its joint, and its body's morph weights.
    /// It samples cleanly, so a row's edit is what breaks it.
    fn animated() -> Self {
        let mut writer = Self::figure_writer();
        let times = |writer: &mut GltfWriter| writer.add_floats("SCALAR", &[0.0, 1.0]);
        let moved = writer.add_floats("VEC3", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let turned = writer.add_floats("VEC4", &[0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]);
        let scaled = writer.add_floats("VEC3", &[1.0, 1.0, 1.0, 2.0, 2.0, 2.0]);
        let weighed = writer.add_floats("SCALAR", &[0.0, 1.0]);
        let mut samplers = Vec::new();
        for output in [moved, turned, scaled, weighed] {
            samplers.push(json!({ "input": times(&mut writer), "output": output }));
        }
        writer.add_animation(json!({
            "name": "Walk",
            "samplers": samplers,
            "channels": [
                { "sampler": TRANSLATION, "target": { "node": 0, "path": "translation" } },
                { "sampler": ROTATION, "target": { "node": 0, "path": "rotation" } },
                { "sampler": SCALE, "target": { "node": 0, "path": "scale" } },
                { "sampler": WEIGHTS, "target": { "node": 1, "path": "weights" } },
            ],
        }));
        Self { doc: writer.document(None), bin: writer.bin().to_vec() }
    }

    fn channel(&mut self, channel: usize) -> &mut Value {
        &mut self.doc["animations"][0]["channels"][channel]
    }

    fn sampler(&mut self, channel: usize) -> &mut Value {
        &mut self.doc["animations"][0]["samplers"][channel]
    }

    /// The key times' accessor of `channel`.
    fn input(&mut self, channel: usize) -> &mut Value {
        let index = Self::index_of(&self.sampler(channel)["input"]);
        self.accessor(index)
    }

    /// The values' accessor of `channel`.
    fn output(&mut self, channel: usize) -> &mut Value {
        let index = Self::index_of(&self.sampler(channel)["output"]);
        self.accessor(index)
    }

    /// Writes `value` over float `at` of the accessor `accessor` picks.
    fn poison(&mut self, accessor: fn(&mut Self) -> &mut Value, at: usize, value: f32) {
        let view = Self::index_of(&accessor(self)["bufferView"]);
        let offset = self.doc["bufferViews"][view]["byteOffset"].as_u64().unwrap_or(0) as usize;
        self.bin[offset + 4 * at..][..4].copy_from_slice(&value.to_le_bytes());
    }
}

/// Samples every animation of `file`, as a cook does.
fn sample_clips(bytes: &[u8]) -> Result<usize, clips::ClipImportError> {
    let (document, buffers, scene) = clips::import_scene(bytes, None)?;
    let settings = crate::clip::ClipSettings::default();
    for index in 0..document.animations().len() {
        clips::sample(&document, &buffers, scene.clip_scene(), index, &settings)?;
    }
    Ok(document.animations().len())
}

/// Runs each case on a fresh [`File::animated`], and fails listing every one that didn't give a
/// clean error, the error of every clip's cook: never a panic.
fn assert_all_clips_fail(cases: &[Case]) {
    let mut wrong = Vec::new();
    for Case { name, craft, says } in cases {
        let mut file = File::animated();
        craft(&mut file);
        let bytes = file.bytes();
        match panic::catch_unwind(AssertUnwindSafe(|| sample_clips(&bytes))) {
            Err(payload) => wrong.push(format!("{name}: panicked: {}", panic_text(&*payload))),
            Ok(Ok(count)) => wrong.push(format!("{name}: sampled {count} clips")),
            Ok(Err(error)) => {
                let text = error.to_string();
                if !text.contains(says) {
                    wrong.push(format!("{name}: said {text:?}, which doesn't mention {says:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {} cases:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

#[test]
fn the_animated_figure_samples_cleanly() {
    assert_eq!(sample_clips(&File::animated().bytes()).expect("it samples"), 1);
}

/// Accessors whose types the `gltf` readers don't expect, or would panic on.
#[test]
fn off_spec_animation_accessors_are_errors() {
    assert_all_clips_fail(&[
        case("times_vec3", |f| f.input(TRANSLATION)["type"] = "VEC3".into(), "input accessor"),
        case(
            "times_u16",
            |f| f.input(TRANSLATION)["componentType"] = UNSIGNED_SHORT.into(),
            "input accessor",
        ),
        case("times_normalized", |f| f.input(TRANSLATION)["normalized"] = true.into(), "input accessor"),
        case(
            "translation_u8",
            |f| f.output(TRANSLATION)["componentType"] = UNSIGNED_BYTE.into(),
            "output accessor",
        ),
        case("translation_scalar", |f| f.output(TRANSLATION)["type"] = "SCALAR".into(), "output accessor"),
        case("translation_vec4", |f| f.output(TRANSLATION)["type"] = "VEC4".into(), "output accessor"),
        case("scale_normalized", |f| f.output(SCALE)["normalized"] = true.into(), "output accessor"),
        case("rotation_vec3", |f| f.output(ROTATION)["type"] = "VEC3".into(), "output accessor"),
        case(
            "rotation_u32",
            |f| f.output(ROTATION)["componentType"] = UNSIGNED_INT.into(),
            "output accessor",
        ),
        case(
            "rotation_integers_unnormalized",
            |f| {
                f.output(ROTATION)["componentType"] = SHORT.into();
            },
            "output accessor",
        ),
        case("weights_vec2", |f| f.output(WEIGHTS)["type"] = "VEC2".into(), "output accessor"),
        case("weights_u32", |f| f.output(WEIGHTS)["componentType"] = UNSIGNED_INT.into(), "output accessor"),
        case(
            "weights_i8_unnormalized",
            |f| f.output(WEIGHTS)["componentType"] = BYTE.into(),
            "output accessor",
        ),
    ]);
}

/// Accessors that claim more than their files hold, where the readers' arithmetic overflows or
/// their indexing would panic.
#[test]
fn animation_accessors_past_their_data_are_errors() {
    assert_all_clips_fail(&[
        case("times_no_elements", |f| f.input(TRANSLATION)["count"] = 0.into(), "no elements"),
        case("times_past_the_view", |f| f.input(TRANSLATION)["count"] = 3.into(), "buffer view"),
        case("times_huge_count", |f| f.input(TRANSLATION)["count"] = HUGE.into(), "budget"),
        case("times_huge_offset", |f| f.input(TRANSLATION)["byteOffset"] = HUGE.into(), "buffer view"),
        case("output_no_elements", |f| f.output(TRANSLATION)["count"] = 0.into(), "no elements"),
        case("output_past_the_view", |f| f.output(TRANSLATION)["count"] = 3.into(), "buffer view"),
        case("output_huge_count", |f| f.output(ROTATION)["count"] = HUGE.into(), "budget"),
        case("output_huge_offset", |f| f.output(SCALE)["byteOffset"] = HUGE.into(), "buffer view"),
        case(
            "output_short_stride",
            |f| {
                let view = File::index_of(&f.output(TRANSLATION)["bufferView"]);
                f.doc["bufferViews"][view]["byteStride"] = 4.into();
            },
            "stride",
        ),
        case(
            "view_past_the_buffer",
            |f| {
                let view = File::index_of(&f.input(ROTATION)["bufferView"]);
                f.doc["bufferViews"][view]["byteOffset"] = 1_000_000.into();
            },
            "past the end of buffer",
        ),
        // The trick that makes a few bytes claim millions: no view of its own, one sparse value.
        case("sparse_times_claiming_much", |f| f.sparse_only(|f| f.input(TRANSLATION), 1 << 40), "budget"),
        case("sparse_values_claiming_much", |f| f.sparse_only(|f| f.output(WEIGHTS), 1 << 40), "budget"),
        case("sparse_over_its_count", |f| f.sparse(|f| f.output(TRANSLATION), 3), "sparse"),
    ]);
}

/// Keys glTF rules out, which the sampler's walk would take on trust.
#[test]
fn bad_keys_are_errors() {
    assert_all_clips_fail(&[
        case("time_nan", |f| f.poison(|f| f.input(TRANSLATION), 1, f32::NAN), "key 1's time"),
        case("time_infinite", |f| f.poison(|f| f.input(ROTATION), 1, f32::INFINITY), "key 1's time"),
        case("time_negative", |f| f.poison(|f| f.input(SCALE), 0, -1.0), "key 0's time"),
        case("time_backward", |f| f.poison(|f| f.input(WEIGHTS), 1, -0.0), "key 1's time"),
        case("time_repeated", |f| f.poison(|f| f.input(TRANSLATION), 1, 0.0), "key 1's time"),
        case("translation_nan", |f| f.poison(|f| f.output(TRANSLATION), 3, f32::NAN), "isn't finite"),
        case("scale_infinite", |f| f.poison(|f| f.output(SCALE), 5, f32::NEG_INFINITY), "isn't finite"),
        case("rotation_nan", |f| f.poison(|f| f.output(ROTATION), 7, f32::NAN), "isn't finite"),
        case(
            "rotation_length_overflow",
            |f| f.poison(|f| f.output(ROTATION), 4, f32::MAX),
            "rotation length isn't finite",
        ),
        case("weight_nan", |f| f.poison(|f| f.output(WEIGHTS), 1, f32::NAN), "isn't finite"),
        case(
            "rotation_with_no_length",
            |f| {
                for at in 4..8 {
                    f.poison(|f| f.output(ROTATION), at, 0.0);
                }
            },
            "key 1's rotation has no length",
        ),
    ]);
}

/// Samplers whose outputs don't fit their keys.
#[test]
fn output_counts_must_fit_keys() {
    assert_all_clips_fail(&[
        case(
            "one_value_for_two_keys",
            |f| f.output(TRANSLATION)["count"] = 1.into(),
            "1 output values for 2 keys",
        ),
        case(
            "one_key_for_two_values",
            |f| f.input(ROTATION)["count"] = 1.into(),
            "2 output values for 1 keys",
        ),
        case(
            "cubic_without_its_tangents",
            |f| f.sampler(TRANSLATION)["interpolation"] = "CUBICSPLINE".into(),
            "where 6 are needed",
        ),
        case(
            "weights_missing_a_target",
            |f| {
                let target = f.primitive()["targets"][0].clone();
                f.primitive()["targets"] = json!([target, target]);
            },
            "where 4 are needed",
        ),
    ]);
}

/// What the document's validation refuses before the importer reads a channel: all of it would
/// panic in the crate's accessors otherwise, and a channel's target only `parse` checks.
#[test]
fn animations_naming_what_isnt_there_are_errors() {
    const INVALID: &str = "not a valid glTF";
    assert_all_clips_fail(&[
        case("channel_sampler_out_of_range", |f| f.channel(0)["sampler"] = 99.into(), INVALID),
        case("channel_node_out_of_range", |f| f.channel(0)["target"]["node"] = 99.into(), INVALID),
        case("channel_unknown_path", |f| f.channel(0)["target"]["path"] = "matrix".into(), INVALID),
        case("sampler_input_out_of_range", |f| f.sampler(0)["input"] = 9999.into(), INVALID),
        case("sampler_output_out_of_range", |f| f.sampler(0)["output"] = 9999.into(), INVALID),
        case("sampler_unknown_interpolation", |f| f.sampler(0)["interpolation"] = "EASE".into(), INVALID),
    ]);
}

/// What a valid document can still get wrong.
#[test]
fn channels_the_scene_cant_honor_are_errors() {
    assert_all_clips_fail(&[
        case("no_channels", |f| f.doc["animations"][0]["channels"] = json!([]), "no channels"),
        case(
            "two_channels_for_one_property",
            |f| {
                let channel = f.channel(TRANSLATION).clone();
                *f.channel(ROTATION) = channel;
            },
            "second channel",
        ),
        case("weights_with_no_mesh", |f| f.channel(WEIGHTS)["target"]["node"] = 0.into(), "no morph targets"),
        case(
            "weights_with_no_morph_targets",
            |f| f.channel(WEIGHTS)["target"]["node"] = 2.into(),
            "no morph targets",
        ),
        // About a year of keys: the length is refused before anything is sampled.
        case("longer_than_a_clip", |f| f.poison(|f| f.input(TRANSLATION), 1, 3.0e7), "a clip holds at most"),
        // Over the samples' budget, though within a u16 of ticks? Only a smaller budget shows it:
        // `budgets_hold_before_data_is_read` in the importer's own tests.
    ]);
}

/// A channel's target is checked for a mesh's cook too, which never reads animations: the crate
/// panics on it whichever way a file comes in, so a broken file says so at the door.
#[test]
fn a_target_that_isnt_there_fails_a_plain_mesh_import_too() {
    let mut file = File::animated();
    file.channel(0)["target"]["node"] = 99.into();
    let error = import_slice(&file.bytes(), None).unwrap_err();
    assert!(error.to_string().contains("not a valid glTF"), "{error}");
}

/// A bad channel is reported wherever it points: one the scene drops is still a bad file, and
/// nothing about it reaches a reader unchecked.
#[test]
fn a_channel_outside_the_scene_is_still_checked() {
    let mut file = File::animated();
    let stray = file.doc["nodes"].as_array().expect("nodes").len();
    file.doc["nodes"].as_array_mut().expect("nodes").push(json!({ "name": "Stray" }));
    file.channel(TRANSLATION)["target"]["node"] = stray.into();
    sample_clips(&file.bytes()).expect("a good channel on a stray node is dropped");
    file.input(TRANSLATION)["count"] = HUGE.into();
    let error = sample_clips(&file.bytes()).unwrap_err();
    assert!(error.to_string().contains("budget"), "{error}");
}

/// Plans and imports `bytes` as a source of animations alone, and samples every clip.
fn sample_animation_source(bytes: &[u8]) -> Result<usize, crate::clip::ClipCookError> {
    use crate::clip::{ClipSettings, source::ClipSource};
    let (names, _) = clips::plan(bytes, |_| Ok(Vec::new()))?;
    let settings = crate::meta::ImportSettings::GltfClips(ClipSettings::default());
    let source = ClipSource::import(Path::new("bad.gltf"), bytes, &settings)?;
    for index in 0..names.len() {
        source.sample(index, &ClipSettings::default())?;
    }
    Ok(names.len())
}

/// A source of animations reads no mesh and no skin's matrices. It reads what's left the same way:
/// the walk of the nodes, and every channel, with nothing unchecked on the way.
#[test]
fn an_animation_source_reads_no_geometry_and_no_less_of_the_rest() {
    let position = |file: &File| -> usize {
        let attributes = &file.doc["meshes"][0]["primitives"][0]["attributes"];
        attributes["POSITION"].as_u64().expect("a position accessor") as usize
    };
    let mut broken_mesh = File::animated();
    let accessor = position(&broken_mesh);
    broken_mesh.doc["accessors"][accessor]["count"] = HUGE.into();
    assert_eq!(sample_animation_source(&broken_mesh.bytes()).expect("the mesh is no concern"), 1);

    type Craft = fn(&mut File);
    let rows: &[(&str, Craft, &str)] = &[
        (
            "two_nodes_in_a_loop",
            |f| {
                f.doc["nodes"][0]["children"] = json!([1]);
                f.doc["nodes"][1]["children"] = json!([0]);
            },
            "reached twice",
        ),
        ("a_node_its_own_child", |f| f.doc["nodes"][0]["children"] = json!([0]), "reached twice"),
        ("a_child_past_the_nodes", |f| f.doc["nodes"][0]["children"] = json!([999]), "not a valid glTF"),
        (
            "no_scene",
            |f| {
                f.doc["scenes"] = json!([]);
                f.doc.as_object_mut().expect("an object").remove("scene");
            },
            "scene",
        ),
        ("a_channel_past_the_samplers", |f| f.channel(ROTATION)["sampler"] = 99.into(), "not a valid glTF"),
        ("a_huge_key_count", |f| f.input(TRANSLATION)["count"] = HUGE.into(), "budget"),
        ("a_nan_in_a_track", |f| f.poison(|f| f.output(TRANSLATION), 3, f32::NAN), "isn't finite"),
        (
            "a_rotation_of_no_length",
            |f| {
                for at in 4..8 {
                    f.poison(|f| f.output(ROTATION), at, 0.0);
                }
            },
            "no length",
        ),
    ];
    let mut wrong = Vec::new();
    for (name, craft, says) in rows {
        let mut file = File::animated();
        craft(&mut file);
        let bytes = file.bytes();
        match panic::catch_unwind(AssertUnwindSafe(|| sample_animation_source(&bytes))) {
            Err(payload) => wrong.push(format!("{name}: panicked: {}", panic_text(&*payload))),
            Ok(Ok(count)) => wrong.push(format!("{name}: sampled {count} clips")),
            Ok(Err(error)) => {
                let mut text = error.to_string();
                let mut cause = std::error::Error::source(&error);
                while let Some(source) = cause {
                    text += &format!(": {source}");
                    cause = source.source();
                }
                if !text.contains(says) {
                    wrong.push(format!("{name}: said {text:?}, which doesn't mention {says:?}"));
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {} rows:\n{}", wrong.len(), rows.len(), wrong.join("\n"));
}

/// Through the cook, the error names the file, as every failure must.
#[test]
fn a_clip_cook_names_the_file_and_the_problem() {
    let dir = TempDir::new("malformed_clip_cook");
    let source = dir.path().join("content/walk.gltf");
    std::fs::create_dir_all(source.parent().expect("a folder")).expect("the folder");
    let mut file = File::animated();
    file.poison(|f| f.output(TRANSLATION), 3, f32::NAN);
    std::fs::write(&source, file.bytes()).expect("write the file");
    let sidecar = crate::meta::AssetMeta::new(
        sapphire_assets::AssetId::from_bytes([0x51; 16]),
        crate::meta::ImportSettings::Gltf(MeshSettings {
            clips: Some(crate::clip::ClipSettings::default()),
            ..MeshSettings::default()
        }),
    );
    let text = sapphire_serialize::to_ron(&sidecar).expect("the sidecar");
    std::fs::write(crate::meta::sidecar_path(&source), text).expect("write the sidecar");
    let out = dir.path().join("out");
    let options = crate::cook::CookOptions { cook: Cook::Desktop, cache: None, locked: true };
    let error = crate::cook::cook_dir(&dir.path().join("content"), &out, &options).unwrap_err();
    assert!(error.to_string().contains("walk.gltf"), "{error}");
    let CookError::Clip { source, .. } = &error else { panic!("{error:?}") };
    assert!(source.to_string().contains("isn't finite"), "{source}");
}

#[test]
fn nonfinite_morph_values_cannot_disappear_as_noise() {
    assert_all_fail(&[
        case(
            "morph_position_nan",
            |f| f.poison(|f| f.target("POSITION"), 0, f32::NAN),
            "morph target 0 POSITION",
        ),
        case("morph_normal_nan", |f| f.poison(|f| f.target("NORMAL"), 0, f32::NAN), "morph target 0 NORMAL"),
    ]);
}
