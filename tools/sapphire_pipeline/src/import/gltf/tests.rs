use std::collections::{BTreeMap, BTreeSet};

use glam::{Quat, Vec4};
use sapphire_assets::material::{AlphaMode, AnimeCharacterMaterial, CharacterRegion, Rim};
use sapphire_assets::mesh::{Bounds, PositionFormat, VertexFlags};
use serde_json::json;

use super::*;
use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry, glb};
use crate::test_util::TempDir;

const EPS: f32 = 1e-5;

fn assert_vec_eq(actual: [f32; 3], expected: Vec3) {
    assert!(Vec3::from(actual).abs_diff_eq(expected, EPS), "{actual:?} != {expected:?}");
}

/// Packs and reads back `mesh`, so every structural invariant of the cooked format is checked.
fn assert_cooks(mesh: &ImportedMesh) {
    let bytes =
        mesh.pack(PositionFormat::Float32, &BTreeMap::new()).expect("pack").to_cooked().expect("cook");
    sapphire_assets::mesh::Mesh::read(&bytes).expect("a valid cooked mesh");
}

/// One slanted triangle whose normal isn't axis-aligned, so non-uniform scale matters.
fn slanted_triangle(with_normals: bool) -> Geometry {
    let normal = Vec3::new(0.0, -1.0, 1.0).normalize().to_array();
    Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 1.0]],
        normals: if with_normals { vec![normal; 3] } else { Vec::new() },
        indices: vec![0, 1, 2],
        ..Geometry::default()
    }
}

/// A document with one node per `(node JSON, geometry)`; each node's `mesh` is filled in.
fn single_mesh_glb(node: serde_json::Value, geometry: &Geometry) -> Vec<u8> {
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Mesh", &[(geometry, None)]);
    let mut node = node;
    node["mesh"] = mesh.into();
    let node = writer.add_node(node);
    writer.add_root(node);
    writer.to_glb()
}

fn face_normal(mesh: &ImportedMesh, triangle: usize) -> Vec3 {
    let [a, b, c] =
        [0, 1, 2].map(|k| Vec3::from(mesh.vertices[mesh.indices[3 * triangle + k] as usize].position));
    (b - a).cross(c - a).normalize()
}

fn submeshes(mesh: &ImportedMesh) -> &[Submesh] {
    &mesh.lods[0].submeshes
}

#[test]
fn applies_hierarchy_transforms_to_positions_and_normals() {
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
    let child = writer.add_node(json!({ "name": "Child", "mesh": mesh, "scale": [1.0, 2.0, 1.0] }));
    let root =
        writer.add_node(json!({ "name": "Root", "translation": [0.0, 0.0, 5.0], "children": [child] }));
    writer.add_root(root);

    let imported = import_slice(&writer.to_glb(), None).unwrap();
    assert_cooks(&imported);
    assert_vec_eq(imported.vertices[2].position, Vec3::new(0.0, 2.0, 6.0));
    // Inverse transpose of diag(1, 2, 1) applied to (0, -1, 1), renormalized.
    let expected = Vec3::new(0.0, -1.0, 2.0).normalize();
    for v in &imported.vertices {
        assert_vec_eq(v.normal, expected);
        assert_vec_eq(v.outline_normal, expected);
    }
    assert!(face_normal(&imported, 0).abs_diff_eq(expected, EPS));
    let bounds = Bounds::enclosing(imported.vertices.iter().map(|v| v.position));
    assert_eq!(bounds, Bounds { min: [0.0, 0.0, 5.0], max: [1.0, 2.0, 6.0] });
    assert_eq!((imported.lods.len(), imported.lods[0].vertex_count, imported.lods[0].error), (1, 3, 0.0));
    assert!(imported.skin.is_none() && imported.morph_targets.is_empty());
    assert_eq!(imported.authored_outline, [false; 3]);
}

#[test]
fn mirrored_nodes_keep_front_faces_outward() {
    let glb = single_mesh_glb(json!({ "scale": [-1.0, 1.0, 1.0] }), &slanted_triangle(true));
    let imported = import_slice(&glb, None).unwrap();
    let normal = Vec3::from(imported.vertices[0].normal);
    assert_vec_eq(normal.to_array(), Vec3::new(0.0, -1.0, 1.0).normalize());
    assert!(face_normal(&imported, 0).dot(normal) > 0.99, "winding was not flipped");
}

#[test]
fn computes_flat_normals_when_missing() {
    let mut quad = slanted_triangle(false);
    quad.positions.push([1.0, 1.0, 1.0]);
    quad.indices.extend([1, 3, 2]);
    let imported = import_slice(&single_mesh_glb(json!({}), &quad), None).unwrap();

    assert_eq!(imported.vertices.len(), 6, "flat shading unwelds every corner");
    assert_eq!(imported.indices, [0, 1, 2, 3, 4, 5]);
    for v in &imported.vertices {
        assert_vec_eq(v.normal, Vec3::new(0.0, -1.0, 1.0).normalize());
    }
}

#[test]
fn reads_vertex_colors_as_linear_data_and_uvs() {
    let mut geometry = slanted_triangle(true);
    geometry.colors = vec![[128, 0, 255, 100]; 3];
    geometry.uvs = vec![[0.0, 0.0], [1.0, 0.0], [0.25, 0.75]];
    let imported = import_slice(&single_mesh_glb(json!({}), &geometry), None).unwrap();
    // Linear data as stored (AO, outline width, outline Z offset); alpha isn't kept.
    assert_eq!(imported.vertices[0].color, [128, 0, 255]);
    assert_eq!(imported.vertices[2].uv, [0.25, 0.75]);

    let plain = import_slice(&single_mesh_glb(json!({}), &slanted_triangle(true)), None).unwrap();
    assert_eq!(plain.vertices[0].color, [255; 3], "no COLOR_0 means unoccluded, the material's outline");
    assert_eq!(plain.vertices[0].flags, VertexFlags::NONE);
    assert_eq!(plain.vertices[0].uv, [0.0, 0.0]);
}

#[test]
fn material_ids_become_vertex_flags() {
    // Two triangles side by side, each its own region.
    let mut two = slanted_triangle(true);
    two.positions.extend([[2.0, 0.0, 0.0], [3.0, 0.0, 0.0], [2.0, 1.0, 1.0]]);
    two.normals.extend(two.normals.clone());
    two.indices = vec![0, 1, 2, 3, 4, 5];
    two.material_ids = vec![3.0, 3.0, 3.0, 7.0, 7.0, 7.0];
    let imported = import_slice(&single_mesh_glb(json!({}), &two), None).unwrap();
    assert_cooks(&imported);
    let ids: Vec<u8> = imported.vertices.iter().map(|v| v.flags.material_id().get()).collect();
    assert_eq!(ids, [3, 3, 3, 7, 7, 7]);
    let mut geometry = slanted_triangle(true);

    // Fractions and IDs past the last are refused.
    for bad in [1.5, 8.0, -1.0] {
        geometry.material_ids = vec![0.0, bad, 1.0];
        let err = import_slice(&single_mesh_glb(json!({}), &geometry), None).unwrap_err();
        assert!(
            matches!(err, GltfImportError::MaterialIdOutOfRange { vertex: 1, value, .. } if value == bad),
            "{err}"
        );
    }

    // A triangle's corners must share their ID: the renderer reads one of them.
    geometry.material_ids = vec![2.0, 2.0, 5.0];
    let err = import_slice(&single_mesh_glb(json!({}), &geometry), None).unwrap_err();
    assert!(matches!(err, GltfImportError::MixedMaterialIds { triangle: 0, ids: [2, 2, 5], .. }), "{err}");

    // Unsigned bytes and shorts are read too; normalized ones and vectors are refused.
    geometry.material_ids = vec![5.0, 5.0, 5.0];
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Mesh", &[(&geometry, None)]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let mut doc = writer.document(None);
    let accessor = doc["meshes"][0]["primitives"][0]["attributes"]["_MATERIAL_ID"].as_u64().unwrap() as usize;
    // Rewrites the IDs as one unsigned byte each, padded to a 4-byte stride as glTF requires.
    let view = doc["accessors"][accessor]["bufferView"].as_u64().unwrap() as usize;
    let offset = doc["bufferViews"][view]["byteOffset"].as_u64().unwrap_or(0) as usize;
    let mut bin = writer.bin().to_vec();
    for (vertex, id) in [5u8; 3].into_iter().enumerate() {
        bin[offset + 4 * vertex..offset + 4 * vertex + 4].copy_from_slice(&[id, 0, 0, 0]);
    }
    doc["bufferViews"][view]["byteStride"] = 4.into();
    doc["accessors"][accessor]["componentType"] = 5121.into(); // UNSIGNED_BYTE
    let imported = import_slice(&glb(&doc, &bin), None).unwrap();
    let ids: Vec<u8> = imported.vertices.iter().map(|v| v.flags.material_id().get()).collect();
    assert_eq!(ids, [5, 5, 5]);
    doc["accessors"][accessor]["normalized"] = true.into();
    let err = import_slice(&glb(&doc, &bin), None).unwrap_err();
    assert!(
        matches!(&err, GltfImportError::UnsupportedAccessor { semantic, .. } if semantic == "_MATERIAL_ID")
    );
}

#[test]
fn maps_materials_and_their_extras() {
    let mut writer = GltfWriter::default();
    let skin = writer.add_material(json!({
        "name": "Skin",
        "pbrMetallicRoughness": { "baseColorFactor": [0.5, 0.25, 0.125, 1.0] },
        "alphaMode": "MASK",
        "alphaCutoff": 0.3,
        "doubleSided": true,
        "extras": {
            "shade_color": [0.1, 0.2, 0.3],
            "outline_color": [0.0, 0.0, 0.0, 0.5],
            "character_shading": [0.2, 0.05, 0.4, 0.006],
            "some_other_blender_property": "ignored",
        },
    }));
    let plain = writer.add_material(json!({ "name": "Plain", "extras": ["not", "an", "object"] }));
    // The depth-offset rim's settings and per-region rim scales (M1 C5).
    let twin = writer.add_material(json!({
        "name": "Plain",
        "alphaMode": "BLEND",
        "extras": {
            "rim": { "width": 3.0, "back_light": 1.0 },
            "rim_regions": [[1.0, 1.0, 1.0], [0.5, 2.0, 0.0]],
        },
    }));
    let unnamed = writer.add_material(json!({}));
    let triangle = slanted_triangle(true);
    let parts = [skin, plain, skin, twin, unnamed].map(|material| (&triangle, Some(material)));
    let mesh = writer.add_mesh("Mesh", &[parts.as_slice(), &[(&triangle, None)]].concat());
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);

    let imported = import_slice(&writer.to_glb(), None).unwrap();
    assert_cooks(&imported);
    let slots: Vec<u32> = submeshes(&imported).iter().map(|s| s.material).collect();
    assert_eq!(slots, [0, 1, 0, 2, 3, 4], "materials are deduplicated in first-use order");
    let names: Vec<&str> = imported.materials.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Skin", "Plain#1", "Plain#2", "#3", "#default"], "slot names are unique");
    let defaults = AnimeCharacterMaterial::default();
    // `doubleSided` draws both faces (M1 C8); glTF's default is one-sided.
    let mut expected = AnimeCharacterMaterial {
        base_color: [0.5, 0.25, 0.125, 1.0],
        alpha: AlphaMode::Mask { cutoff: 0.3 },
        double_sided: true,
        regions: vec![CharacterRegion {
            shade_color: [0.1, 0.2, 0.3],
            outline_color: [0.0, 0.0, 0.0],
            ..CharacterRegion::default()
        }],
        ..defaults.clone()
    };
    expected.light_split.threshold = 0.2;
    expected.light_split.softness = 0.05;
    expected.rim.strength = 0.4;
    expected.outline.width = 0.006;
    assert_eq!(imported.materials[0].generated, expected);
    let white = AnimeCharacterMaterial { base_color: [1.0; 4], ..defaults.clone() };
    assert_eq!(imported.materials[1].generated, white);
    let twin = &imported.materials[2].generated;
    assert_eq!(twin.alpha, AlphaMode::Blend);
    assert_eq!(twin.rim, Rim { width: 3.0, back_light: 1.0, ..Rim::default() });
    let region = CharacterRegion::default();
    let masked = CharacterRegion { rim_strength: 0.5, rim_width: 2.0, rim_overlap: 0.0, ..region };
    assert_eq!(twin.regions, [region, masked], "regions past the first copy region 0 but for their rims");
    // The glTF default material is white.
    assert_eq!(imported.materials[4].generated, white);
}

#[test]
fn slot_names_stay_unique_when_names_look_made_up() {
    let mut writer = GltfWriter::default();
    let materials = [
        json!({ "name": "Plain" }),
        json!({ "name": "Plain", "alphaMode": "MASK", "alphaCutoff": 3.0 }),
        json!({ "name": "Plain#1" }),
        json!({ "name": "#default" }),
    ]
    .map(|material| writer.add_material(material));
    let triangle = slanted_triangle(true);
    let parts: Vec<_> = materials.iter().map(|&material| (&triangle, Some(material))).collect();
    let mesh = writer.add_mesh("Mesh", &[parts.as_slice(), &[(&triangle, None)]].concat());
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);

    let imported = import_slice(&writer.to_glb(), None).unwrap();
    let names: Vec<&str> = imported.materials.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Plain#0", "Plain#1#1", "Plain#1#2", "#default#3", "#default"]);
    // glTF allows any cutoff from 0; above 1 is the same as 1.
    assert_eq!(imported.materials[1].generated.alpha, AlphaMode::Mask { cutoff: 1.0 });
}

#[test]
fn rejects_malformed_material_extras() {
    let mut writer = GltfWriter::default();
    let material = writer.add_material(json!({ "extras": { "rim": { "widht": 3.0 } } }));
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), Some(material))]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let err = import_slice(&writer.to_glb(), None).unwrap_err();
    assert!(matches!(err, GltfImportError::MaterialExtras { .. }), "a misspelled rim setting: {err:?}");

    let mut writer = GltfWriter::default();
    let material =
        writer.add_material(json!({ "name": "Broken", "extras": { "character_shading": [0.1, 0.2, 0.3] } }));
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), Some(material))]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);

    let err = import_slice(&writer.to_glb(), None).unwrap_err();
    assert!(matches!(&err, GltfImportError::MaterialExtras { material, .. } if material == "\"Broken\""));
    assert_eq!(err.to_string(), "material \"Broken\": invalid material extras");

    let mut writer = GltfWriter::default();
    let material = writer.add_material(json!({ "extras": { "shade_color": [0.1, 0.2] } }));
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), Some(material))]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let err = import_slice(&writer.to_glb(), None).unwrap_err();
    let GltfImportError::MaterialExtras { material, source } = err else { panic!("{err:?}") };
    assert_eq!(material, "#0");
    assert!(source.to_string().contains("3 (RGB) or 4 (RGBA)"), "{source}");
}

#[test]
fn rejects_unsupported_input() {
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Points", &[(&slanted_triangle(true), None)]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let mut doc = writer.document(None);
    doc["meshes"][0]["primitives"][0]["mode"] = 0.into();
    let err = import_slice(&glb(&doc, writer.bin()), None).unwrap_err();
    assert!(matches!(err, GltfImportError::UnsupportedMode { mode: Mode::Points, .. }), "{err:?}");
    assert_eq!(
        err.to_string(),
        "mesh \"Points\" primitive 0: primitive mode Points is not supported; export triangles"
    );

    let mut bad_index = slanted_triangle(true);
    bad_index.indices = vec![0, 1, 7];
    let err = import_slice(&single_mesh_glb(json!({}), &bad_index), None).unwrap_err();
    assert!(matches!(err, GltfImportError::IndexOutOfRange { index: 7, vertex_count: 3, .. }));

    let mut empty = GltfWriter::default();
    let node = empty.add_node(json!({ "name": "Empty" }));
    empty.add_root(node);
    assert!(matches!(import_slice(&empty.to_glb(), None), Err(GltfImportError::NoGeometry)));

    assert!(matches!(import_slice(b"not gltf", None), Err(GltfImportError::Parse(_))));
}

#[test]
fn rejects_node_cycles() {
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
    let a = writer.add_node(json!({ "mesh": mesh, "children": [1] }));
    writer.add_node(json!({ "children": [0] }));
    writer.add_root(a);
    assert!(matches!(import_slice(&writer.to_glb(), None), Err(GltfImportError::NodeCycle { node: 0 })));
}

#[test]
fn imports_gltf_with_external_buffer() {
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let json = serde_json::to_vec(&writer.document(Some("triangle.bin"))).unwrap();

    let dir = TempDir::new("external_buffer");
    std::fs::write(dir.path().join("triangle.gltf"), &json).unwrap();
    std::fs::write(dir.path().join("triangle.bin"), writer.bin()).unwrap();

    let imported = import(&dir.path().join("triangle.gltf")).unwrap();
    assert_eq!(imported.vertices.len(), 3);
    assert_eq!(external_files(&json).unwrap(), ["triangle.bin"]);
    assert!(external_files(&writer.to_glb()).unwrap().is_empty(), "GLB buffers are internal");
    // Without a base directory the external reference can't resolve.
    assert!(matches!(import_slice(&json, None), Err(GltfImportError::Buffer { index: 0, .. })));
    assert!(matches!(import(&dir.path().join("missing.gltf")), Err(GltfImportError::Io { .. })));
}

#[test]
fn decodes_data_uris_and_escaped_file_names() {
    assert_eq!(
        decode_data_uri("application/octet-stream;base64,AAECAw==").as_deref(),
        Some(&[0, 1, 2, 3][..])
    );
    assert_eq!(decode_data_uri(";base64,TWFu").as_deref(), Some(&b"Man"[..]));
    assert_eq!(decode_data_uri("text/plain,hello"), None, "not base64");
    assert_eq!(decode_data_uri(";base64,AA!A"), None, "bad character");
    assert_eq!(decode_data_uri(";base64,TWFuA"), None, "length 1 mod 4");
    assert_eq!(percent_decode("my%20mesh.bin").as_deref(), Some("my mesh.bin"));
    assert_eq!(percent_decode("bad%2"), None);

    // A whole document with its buffer inline.
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Mesh", &[(&slanted_triangle(true), None)]);
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let mut document = writer.document(Some("unused.bin"));
    document["buffers"][0]["uri"] =
        json!(format!("data:application/octet-stream;base64,{}", base64(writer.bin())));
    let bytes = serde_json::to_vec(&document).unwrap();
    let imported = import_slice(&bytes, None).unwrap();
    assert_eq!(imported.vertices.len(), 3);
    assert!(external_files(&bytes).unwrap().is_empty(), "data URIs are inline");
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().enumerate().fold(0u32, |word, (i, &b)| word | u32::from(b) << (16 - 8 * i));
        for i in 0..=chunk.len() {
            out.push(ALPHABET[(word >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    while !out.len().is_multiple_of(4) {
        out.push('=');
    }
    out
}

#[test]
fn authored_outline_normals_are_read_transformed_and_flagged() {
    let mut writer = GltfWriter::default();
    let mut authored = slanted_triangle(true);
    authored.outline_normals = vec![[1.0, 0.0, 0.0], [0.0, 2.0, 0.0], [0.0, 1.0, 1.0]];
    let mesh = writer.add_mesh("Mesh", &[(&authored, None), (&slanted_triangle(true), None)]);
    let node = writer.add_node(json!({ "mesh": mesh, "scale": [1.0, 2.0, 1.0] }));
    writer.add_root(node);

    let imported = import_slice(&writer.to_glb(), None).unwrap();
    assert_eq!(imported.authored_outline, [true, true, true, false, false, false]);
    // Normalized, and transformed like NORMAL: the inverse transpose of diag(1, 2, 1).
    assert_vec_eq(imported.vertices[0].outline_normal, Vec3::X);
    assert_vec_eq(imported.vertices[1].outline_normal, Vec3::Y);
    assert_vec_eq(imported.vertices[2].outline_normal, Vec3::new(0.0, 0.5, 1.0).normalize());
    // Without one, the outline normal starts as the shading normal.
    assert_eq!(imported.vertices[3].outline_normal, imported.vertices[3].normal);

    // Normalized shorts are read too; other encodings are refused.
    let mut doc = writer.document(None);
    let accessor =
        doc["meshes"][0]["primitives"][0]["attributes"]["_OUTLINE_NORMAL"].as_u64().unwrap() as usize;
    doc["accessors"][accessor]["componentType"] = 5121.into(); // UNSIGNED_BYTE
    let err = import_slice(&glb(&doc, writer.bin()), None).unwrap_err();
    assert!(
        matches!(&err, GltfImportError::UnsupportedAccessor { semantic, .. } if semantic == "_OUTLINE_NORMAL")
    );
}

/// The skin test's skeleton: `Armature → Hips → {Spine → Head, LegL}`, plus a skinned `Body`
/// beside `Hips`. Returns the document and the world rest transforms of
/// `[Armature, Hips, Spine, Head, LegL]`.
fn skinned_figure(geometry: &Geometry) -> (GltfWriter, [Mat4; 5]) {
    let mut writer = GltfWriter::default();
    let rotation = Quat::from_rotation_y(0.3);
    let local = [
        Mat4::from_translation(Vec3::new(0.0, 0.0, 2.0)),
        Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0)),
        Mat4::from_rotation_translation(rotation, Vec3::new(0.0, 0.3, 0.0)),
        Mat4::from_translation(Vec3::new(0.0, 0.4, 0.0)),
        Mat4::from_scale_rotation_translation(Vec3::splat(1.5), Quat::IDENTITY, Vec3::new(0.1, -0.5, 0.0)),
    ];
    // Node order in the file differs from the skeleton's depth-first order on purpose.
    let head = writer.add_node(json!({ "name": "Head", "translation": [0.0, 0.4, 0.0] }));
    let spine = writer.add_node(json!({
        "name": "Spine", "translation": [0.0, 0.3, 0.0], "rotation": rotation.to_array(), "children": [head],
    }));
    let leg =
        writer.add_node(json!({ "name": "LegL", "translation": [0.1, -0.5, 0.0], "scale": [1.5, 1.5, 1.5] }));
    let hips =
        writer.add_node(json!({ "name": "Hips", "translation": [0.0, 1.0, 0.0], "children": [spine, leg] }));
    let world = [
        local[0],
        local[0] * local[1],
        local[0] * local[1] * local[2],
        local[0] * local[1] * local[2] * local[3],
        local[0] * local[1] * local[4],
    ];
    // The skin lists its joints in yet another order.
    let skin_joints = [head, hips, leg, spine];
    let inverse_binds = [world[3], world[1], world[4], world[2]].map(|m| m.inverse().to_cols_array());
    let skin = writer.add_skin(&skin_joints, Some(&inverse_binds));
    let mesh = writer.add_mesh("Body", &[(geometry, None)]);
    // The skinned node's own transform must be ignored.
    let body = writer
        .add_node(json!({ "name": "Body", "mesh": mesh, "skin": skin, "translation": [9.0, 9.0, 9.0] }));
    let armature = writer
        .add_node(json!({ "name": "Armature", "translation": [0.0, 0.0, 2.0], "children": [hips, body] }));
    writer.add_root(armature);
    (writer, world)
}

fn skinned_triangle() -> Geometry {
    let mut geometry = slanted_triangle(true);
    geometry.influences = vec![InfluenceSet {
        // Skin joint indices: 0 Head, 1 Hips, 2 LegL, 3 Spine.
        joints: vec![[0, 1, 0, 0], [3, 0, 0, 0], [2, 1, 3, 0]],
        // Unnormalized on purpose: the second vertex sums to 2.
        weights: vec![[0.25, 0.75, 0.0, 0.0], [2.0, 0.0, 0.0, 0.0], [0.5, 0.25, 0.25, 0.0]],
    }];
    geometry
}

#[test]
fn imports_skins_with_joint_order_inverse_binds_and_hierarchy() {
    let geometry = skinned_triangle();
    let (writer, world) = skinned_figure(&geometry);
    let imported = import_slice(&writer.to_glb(), None).unwrap();
    assert_cooks(&imported);
    let skin = imported.skin.as_ref().expect("a skinned mesh");

    // Depth-first from the scene root, children in file order; Body isn't a joint.
    let names: Vec<&str> = skin.joints.iter().map(|joint| joint.name.as_str()).collect();
    assert_eq!(names, ["Armature", "Hips", "Spine", "Head", "LegL"]);
    let parents: Vec<Option<u16>> = skin.joints.iter().map(|joint| joint.parent).collect();
    assert_eq!(parents, [None, Some(0), Some(1), Some(2), Some(1)]);
    assert_eq!(skin.joints[4].rest.translation, [0.1, -0.5, 0.0]);
    assert_eq!(skin.joints[4].rest.scale, [1.5; 3]);
    assert!(Vec4::from(skin.joints[2].rest.rotation).abs_diff_eq(Quat::from_rotation_y(0.3).into(), EPS));

    // The palette follows the skin's joint order, with its inverse bind matrices.
    let joints: Vec<u16> = skin.binds.iter().map(|bind| bind.joint).collect();
    assert_eq!(joints, [3, 1, 4, 2]);
    for (bind, joint) in skin.binds.iter().zip([3, 1, 4, 2]) {
        let palette = world[joint] * Mat4::from_cols_array_2d(&bind.inverse_bind_matrix);
        assert!(palette.abs_diff_eq(Mat4::IDENTITY, EPS), "joint {joint}: rest × inverse bind = {palette}");
    }

    // Weights map to palette entries, heaviest first, normalized.
    assert_eq!(skin.weights.len(), 3);
    assert_eq!((skin.weights[0].binds, skin.weights[0].weights), ([1, 0, 0, 0], [0.75, 0.25, 0.0, 0.0]));
    assert_eq!((skin.weights[1].binds, skin.weights[1].weights), ([3, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
    assert_eq!(skin.weights[2].binds[..3], [2, 1, 3]);
    for weights in &skin.weights {
        assert!((weights.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }
    // Bind space: the Body node's translation and the armature's are not applied.
    assert_eq!(imported.vertices[2].position, [0.0, 1.0, 1.0]);
    assert_vec_eq(imported.vertices[0].normal, Vec3::new(0.0, -1.0, 1.0).normalize());
}

#[test]
fn keeps_the_four_heaviest_of_more_influences_and_binds_unweighted_vertices_to_the_first_joint() {
    let mut geometry = slanted_triangle(true);
    geometry.influences = vec![
        InfluenceSet {
            joints: vec![[0, 1, 2, 3], [0, 0, 0, 0], [1, 0, 0, 0]],
            weights: vec![[0.1, 0.3, 0.05, 0.2], [0.0; 4], [1.0, 0.0, 0.0, 0.0]],
        },
        InfluenceSet {
            joints: vec![[3, 2, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]],
            weights: vec![[0.15, 0.2, 0.0, 0.0], [0.0; 4], [0.0; 4]],
        },
    ];
    let (writer, _) = skinned_figure(&geometry);
    let imported = import_slice(&writer.to_glb(), None).unwrap();
    let skin = imported.skin.as_ref().unwrap();
    // Six influences over two sets name four joints, and repeats merge: joint 3 (Spine,
    // palette 3) has 0.35 in all, joint 1 (Hips) 0.3, joint 2 (LegL) 0.25 and joint 0 (Head)
    // 0.1. (Dropping beyond four is `SkinWeights::strongest`'s test.)
    assert_eq!(skin.weights[0].binds, [3, 1, 2, 0]);
    let total = 0.35 + 0.3 + 0.25 + 0.1;
    for (actual, expected) in skin.weights[0].weights.iter().zip([0.35, 0.3, 0.25, 0.1]) {
        assert!((actual - expected / total).abs() < 1e-6, "{:?}", skin.weights[0]);
    }
    // No positive weight: all on the skin's first joint (Head, palette 0).
    assert_eq!((skin.weights[1].binds, skin.weights[1].weights), ([0, 0, 0, 0], [1.0, 0.0, 0.0, 0.0]));
    assert_eq!(skin.weights[2].binds[0], 1);
}

#[test]
fn static_parts_of_a_skinned_scene_bind_rigidly() {
    let (mut writer, world) = skinned_figure(&skinned_triangle());
    // A sword in the head's hand and a prop with no joint above it.
    let sword_mesh = writer.add_mesh("Sword", &[(&slanted_triangle(true), None)]);
    let sword =
        writer.add_node(json!({ "name": "Sword", "mesh": sword_mesh, "translation": [0.0, 0.1, 0.0] }));
    let prop = writer.add_node(json!({ "name": "Prop", "mesh": sword_mesh, "translation": [5.0, 0.0, 0.0] }));
    let mut doc = writer.document(None);
    // Parent the sword to Head (node 0); the prop is a second root beside the armature (5).
    doc["nodes"][0]["children"] = json!([sword]);
    doc["scenes"][0]["nodes"] = json!([5, prop]);
    let imported = import_slice(&glb(&doc, writer.bin()), None).unwrap();
    assert_cooks(&imported);
    let skin = imported.skin.as_ref().unwrap();
    // Depth-first, the sword (under Head) comes before Body: vertices 0–2 are the sword's,
    // 3–5 the body's and 6–8 the prop's. The sword binds to Head (joint 3), the prop to the
    // first root (Armature, joint 0).
    let sword_bind = skin.binds[usize::from(skin.weights[0].binds[0])];
    let prop_bind = skin.binds[usize::from(skin.weights[6].binds[0])];
    assert_eq!((sword_bind.joint, prop_bind.joint), (3, 0));
    for (bind, rest) in [(sword_bind, world[3]), (prop_bind, world[0])] {
        let palette = rest * Mat4::from_cols_array_2d(&bind.inverse_bind_matrix);
        assert!(palette.abs_diff_eq(Mat4::IDENTITY, EPS), "{palette}");
    }
    // Static parts keep their world transform: the sword hangs below the head.
    let head_origin = world[3].transform_point3(Vec3::ZERO);
    assert_vec_eq(imported.vertices[0].position, head_origin + Vec3::new(0.0, 0.1, 0.0));
    assert_eq!(skin.weights[0].weights, [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(skin.weights[3].binds[0], 1, "the body is still skinned");
}

#[test]
fn rejects_broken_skins() {
    let mut out_of_range = skinned_triangle();
    out_of_range.influences[0].joints[1] = [7, 0, 0, 0];
    let (writer, _) = skinned_figure(&out_of_range);
    let err = import_slice(&writer.to_glb(), None).unwrap_err();
    assert!(
        matches!(err, GltfImportError::JointOutOfRange { vertex: 1, joint: 7, joints: 4, .. }),
        "{err:?}"
    );

    let (writer, _) = skinned_figure(&skinned_triangle());
    let mut doc = writer.document(None);
    let matrices = doc["skins"][0]["inverseBindMatrices"].as_u64().unwrap() as usize;
    doc["accessors"][matrices]["count"] = 3.into();
    let err = import_slice(&glb(&doc, writer.bin()), None).unwrap_err();
    assert!(matches!(err, GltfImportError::InverseBindMatrices { found: 3, expected: 4, .. }), "{err:?}");

    let mut doc = writer.document(None);
    doc["nodes"].as_array_mut().unwrap().push(json!({ "name": "Elsewhere" }));
    doc["skins"][0]["joints"][0] = 6.into();
    let err = import_slice(&glb(&doc, writer.bin()), None).unwrap_err();
    assert!(matches!(err, GltfImportError::JointNotInScene { node: 6, .. }), "{err:?}");

    let mut doc = writer.document(None);
    let joints = doc["meshes"][0]["primitives"][0]["attributes"]["JOINTS_0"].as_u64().unwrap() as usize;
    doc["accessors"][joints]["type"] = "VEC3".into();
    let err = import_slice(&glb(&doc, writer.bin()), None).unwrap_err();
    assert!(matches!(&err, GltfImportError::UnsupportedAccessor { semantic, .. } if semantic == "JOINTS_0"));
}

#[test]
fn imports_named_sparse_morph_targets_merged_across_meshes() {
    let mut face = slanted_triangle(true);
    face.targets = vec![
        // "blink" moves vertex 1 and turns vertex 2's normal.
        MorphGeometry {
            positions: vec![[0.0; 3], [0.0, 0.1, 0.0], [0.0; 3]],
            normals: vec![[0.0; 3], [0.0; 3], [0.0, 1.0, 0.0]],
        },
        // "smile" moves nothing measurable.
        MorphGeometry { positions: vec![[1e-8, 0.0, 0.0]; 3], normals: Vec::new() },
    ];
    let mut lashes = slanted_triangle(true);
    lashes.targets = vec![MorphGeometry { positions: vec![[0.0, 0.0, 0.2]; 3], normals: Vec::new() }];
    let mut unnamed = slanted_triangle(true);
    unnamed.targets = vec![MorphGeometry { positions: vec![[0.5, 0.0, 0.0]; 3], normals: Vec::new() }];

    let mut writer = GltfWriter::default();
    let face_mesh = writer.add_mesh("Face", &[(&face, None)]);
    writer.set_mesh_field(face_mesh, "extras", json!({ "targetNames": ["blink", "smile"] }));
    writer.set_mesh_field(face_mesh, "weights", json!([0.0, 0.5]));
    let lashes_mesh = writer.add_mesh("Lashes", &[(&lashes, None)]);
    writer.set_mesh_field(lashes_mesh, "extras", json!({ "targetNames": ["blink"] }));
    let unnamed_mesh = writer.add_mesh("Prop", &[(&unnamed, None)]);
    // The face is scaled 2× along y: deltas take the linear transform.
    let face_node =
        writer.add_node(json!({ "mesh": face_mesh, "scale": [1.0, 2.0, 1.0], "weights": [0.25, 0.0] }));
    let lashes_node = writer.add_node(json!({ "mesh": lashes_mesh, "translation": [3.0, 0.0, 0.0] }));
    let unnamed_node = writer.add_node(json!({ "mesh": unnamed_mesh }));
    for node in [face_node, lashes_node, unnamed_node] {
        writer.add_root(node);
    }

    let imported = import_slice(&writer.to_glb(), None).unwrap();
    assert_cooks(&imported);
    let names: Vec<&str> = imported.morph_targets.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["blink", "smile", "Prop/0"]);
    let [blink, smile, prop] = &imported.morph_targets[..] else { unreachable!() };
    // The node's weights win over the mesh's.
    assert_eq!((blink.default_weight, smile.default_weight), (0.25, 0.0));
    assert!(smile.deltas.is_empty(), "sub-micrometer deltas are dropped");
    // Face vertices 1 and 2, then all three lash vertices (3, 4, 5), in vertex order.
    let vertices: Vec<u32> = blink.deltas.iter().map(|d| d.vertex).collect();
    assert_eq!(vertices, [1, 2, 3, 4, 5]);
    assert_vec_eq(blink.deltas[0].position, Vec3::new(0.0, 0.2, 0.0));
    assert_vec_eq(blink.deltas[2].position, Vec3::new(0.0, 0.0, 0.2));
    // The normal delta reaches the transformed, normalized full-weight normal.
    let base = Vec3::from(imported.vertices[2].normal);
    let normal_matrix = Mat3::from_diagonal(Vec3::new(1.0, 0.5, 1.0));
    let target = (normal_matrix * (Vec3::new(0.0, -1.0, 1.0).normalize() + Vec3::Y)).normalize();
    assert_vec_eq((base + Vec3::from(blink.deltas[1].normal)).to_array(), target);
    assert_eq!(prop.deltas.len(), 3);
}

/// The review's AP-003 fixture: one mesh, a flat triangle whose `raise` target lifts it 1 m,
/// instanced by a node per `(name, x, default weight)`, in that order.
fn raised_instances(instances: &[(&str, f32, f32)]) -> Vec<u8> {
    let triangle = Geometry {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
        indices: vec![0, 1, 2],
        targets: vec![MorphGeometry { positions: vec![[0.0, 1.0, 0.0]; 3], normals: Vec::new() }],
        ..Geometry::default()
    };
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Panel", &[(&triangle, None)]);
    writer.set_mesh_field(mesh, "extras", json!({ "targetNames": ["raise"] }));
    for &(name, x, weight) in instances {
        let node = writer.add_node(
            json!({ "name": name, "mesh": mesh, "translation": [x, 0.0, 0.0], "weights": [weight] }),
        );
        writer.add_root(node);
    }
    writer.to_glb()
}

/// Every vertex where `mesh`'s targets rest it, at their default weights.
fn at_rest(mesh: &ImportedMesh) -> Vec<Vec3> {
    let mut positions: Vec<Vec3> = mesh.vertices.iter().map(|v| Vec3::from(v.position)).collect();
    for target in &mesh.morph_targets {
        for delta in &target.deltas {
            positions[delta.vertex as usize] += target.default_weight * Vec3::from(delta.position);
        }
    }
    positions
}

#[test]
fn instances_of_one_mesh_keep_their_own_morph_defaults() {
    // Before AP-003 both instances merged into one `raise` at the first one's weight, 0: the
    // second instance lost its raised pose.
    let imported = import_slice(&raised_instances(&[("Left", 0.0, 0.0), ("Right", 2.0, 1.0)]), None).unwrap();
    assert_cooks(&imported);
    let targets: Vec<(&str, f32, usize)> = imported
        .morph_targets
        .iter()
        .map(|target| (target.name.as_str(), target.default_weight, target.deltas.len()))
        .collect();
    assert_eq!(targets, [("raise", 0.0, 3), ("raise@Right", 1.0, 3)]);
    let heights: Vec<f32> = at_rest(&imported).iter().map(|p| p.y).collect();
    assert_eq!(heights, [0.0, 0.0, 0.0, 1.0, 1.0, 1.0], "the first triangle rests at 0, the second at 1");

    // The scene's order picks which keeps the plain name, not where anything rests.
    let reversed = import_slice(&raised_instances(&[("Right", 2.0, 1.0), ("Left", 0.0, 0.0)]), None).unwrap();
    let names: Vec<&str> = reversed.morph_targets.iter().map(|target| target.name.as_str()).collect();
    assert_eq!(names, ["raise", "raise@Left"]);
    let rest = |mesh: &ImportedMesh| -> BTreeSet<[u32; 3]> {
        at_rest(mesh).iter().map(|p| p.to_array().map(f32::to_bits)).collect()
    };
    assert_eq!(rest(&reversed), rest(&imported), "the same world geometry at rest");

    // Instances at one weight still share a target, and a taken name gets a number.
    let imported = import_slice(
        &raised_instances(&[
            ("Left", 0.0, 1.0),
            ("Right", 2.0, 0.5),
            ("Middle", 4.0, 1.0),
            ("Right", 6.0, 0.25),
        ]),
        None,
    )
    .unwrap();
    let targets: Vec<(&str, f32, usize)> = imported
        .morph_targets
        .iter()
        .map(|target| (target.name.as_str(), target.default_weight, target.deltas.len()))
        .collect();
    assert_eq!(targets, [("raise", 1.0, 6), ("raise@Right", 0.5, 3), ("raise@Right#2", 0.25, 3)]);
}

#[test]
fn rejects_two_morph_targets_with_one_name_in_a_primitive() {
    let mut face = slanted_triangle(true);
    let moved = MorphGeometry { positions: vec![[0.0, 0.1, 0.0]; 3], normals: Vec::new() };
    face.targets = vec![moved.clone(), moved];
    let mut writer = GltfWriter::default();
    let mesh = writer.add_mesh("Face", &[(&face, None)]);
    writer.set_mesh_field(mesh, "extras", json!({ "targetNames": ["blink", "blink"] }));
    let node = writer.add_node(json!({ "mesh": mesh }));
    writer.add_root(node);
    let err = import_slice(&writer.to_glb(), None).unwrap_err();
    assert!(matches!(&err, GltfImportError::DuplicateMorphTarget { name, .. } if name == "blink"), "{err:?}");
}
