//! The Blender add-on's test character (M6 C3): `tools/blender/test_character.py` builds it with the add-on and
//! exports it through the add-on's export into `fixtures/character/`, committed. Blender isn't on CI, so this is
//! what CI holds of the add-on: the importer reads back every attribute the script set (the anime data in glTF's
//! terms, `import/gltf.rs`), and `check` judges the character at the hero role. A change to the script or the
//! add-on's export regenerates the file (the script's docs say how) and is held here.
//!
//! glTF is Y-up: the script's Blender (x, y, z) is (x, z, -y) here, so the figure faces +Z.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use sapphire_pipeline::check::{self, Role, Rule, Severity};
use sapphire_pipeline::import::gltf;
use sapphire_pipeline::mesh::ImportedMesh;

fn folder() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/character")
}

fn character() -> ImportedMesh {
    gltf::import(&folder().join("test_character.glb")).unwrap()
}

/// The vertices of `slot`'s triangles, three to a triangle.
fn triangles(mesh: &ImportedMesh, slot: &str) -> Vec<[usize; 3]> {
    let material = mesh.materials.iter().position(|material| material.name == slot).unwrap() as u32;
    let mut found = Vec::new();
    for submesh in mesh.lods[0].submeshes.iter().filter(|submesh| submesh.material == material) {
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        let (corners, rest) = mesh.indices[range].as_chunks::<3>();
        assert!(rest.is_empty(), "{slot}'s indices are whole triangles");
        found.extend(corners.iter().map(|&[a, b, c]| [a as usize, b as usize, c as usize]));
    }
    assert!(!found.is_empty(), "no triangles in {slot}");
    found
}

fn vertices(triangles: &[[usize; 3]]) -> BTreeSet<usize> {
    triangles.iter().flatten().copied().collect()
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn unit(a: [f32; 3]) -> [f32; 3] {
    let length = dot(a, a).sqrt();
    [a[0] / length, a[1] / length, a[2] / length]
}

fn close(a: &[f32], b: &[f32]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| (a - b).abs() < 1e-6)
}

#[test]
fn every_slot_carries_the_extras_and_base_color_the_script_set() {
    let mesh = character();
    let names: BTreeSet<&str> = mesh.materials.iter().map(|material| material.name.as_str()).collect();
    assert_eq!(names, BTreeSet::from(["Cloth", "Face", "Hair", "Skin"]));
    let slot = |name: &str| &mesh.materials.iter().find(|material| material.name == name).unwrap().generated;

    let skin = slot("Skin");
    assert!(close(&skin.base_color, &[0.93, 0.76, 0.68, 1.0]), "{:?}", skin.base_color);
    assert!(close(&skin.regions[0].shade_color, &[0.85, 0.55, 0.5]));
    assert!(close(&[skin.light_split.threshold, skin.light_split.softness], &[0.05, 0.03]));
    assert!(close(&[skin.rim.strength, skin.outline.width], &[0.4, 5.0]));

    let cloth = slot("Cloth");
    assert_eq!(cloth.regions.len(), 2, "a region a rim region");
    assert!(close(
        &[cloth.regions[0].rim_strength, cloth.regions[0].rim_width, cloth.regions[0].rim_overlap],
        &[1.0; 3]
    ));
    assert!(close(
        &[cloth.regions[1].rim_strength, cloth.regions[1].rim_width, cloth.regions[1].rim_overlap],
        &[1.5, 0.8, 0.5]
    ));
    for region in &cloth.regions {
        assert!(close(&region.shade_color, &[0.4, 0.42, 0.6]), "region 0's shade, copied to each region");
    }

    let face = slot("Face");
    assert!(close(&face.regions[0].shade_color, &[0.9, 0.62, 0.58]));
    assert!(close(&face.regions[0].outline_color, &[0.35, 0.18, 0.16]));
    assert!(close(
        &[face.light_split.threshold, face.light_split.softness, face.rim.strength, face.outline.width],
        &[0.1, 0.02, 0.3, 3.5]
    ));

    let hair = slot("Hair");
    assert!(close(&hair.regions[0].outline_color, &[0.12, 0.08, 0.2]));
    let rim = &hair.rim;
    assert!(close(
        &[rim.width, rim.strength, rim.threshold, rim.fresnel, rim.light_gate, rim.back_light],
        &[3.0, 0.5, 0.12, 0.6, 0.7, 0.2]
    ));

    // Back-face culling on in every material: Blender's own default would export each as two-sided.
    assert!(mesh.materials.iter().all(|material| !material.generated.double_sided));
}

#[test]
fn each_vertex_carries_its_parts_data_color() {
    let mesh = character();
    let colors = |slot: &str| -> BTreeSet<[u8; 3]> {
        vertices(&triangles(&mesh, slot)).into_iter().map(|vertex| mesh.vertices[vertex].color).collect()
    };
    // Bytes over 255, as the script writes them, reach the vertex exactly.
    assert_eq!(colors("Skin"), BTreeSet::from([[255, 255, 0]]));
    assert_eq!(colors("Face"), BTreeSet::from([[255, 128, 64]]));
    assert_eq!(colors("Hair"), BTreeSet::from([[230, 255, 102]]));
    // The cloth is (255, 255, 0) but at the skirt's hem ring, 0.66 m up.
    let mut hem = 0;
    for vertex in vertices(&triangles(&mesh, "Cloth")) {
        let vertex = &mesh.vertices[vertex];
        if (vertex.position[1] - 0.66).abs() < 1e-4 {
            assert_eq!(vertex.color, [153, 204, 0]);
            hem += 1;
        } else {
            assert_eq!(vertex.color, [255, 255, 0], "{:?}", vertex.position);
        }
    }
    assert!(hem >= 12, "the hem ring's 12 vertices, and its UV seam's: {hem}");
}

#[test]
fn material_ids_are_painted_on_whole_faces() {
    let mesh = character();
    let ids = |slot: &str, band: f32, id: u8| {
        let mut banded = 0;
        for triangle in triangles(&mesh, slot) {
            let found: BTreeSet<u8> =
                triangle.iter().map(|&vertex| mesh.vertices[vertex].flags.material_id().get()).collect();
            assert_eq!(found.len(), 1, "a triangle's vertices share its ID");
            let height = triangle.iter().map(|&vertex| mesh.vertices[vertex].position[1]).sum::<f32>() / 3.0;
            let expected = if height < band { id } else { 0 };
            assert_eq!(found.into_iter().next(), Some(expected), "{slot} at {height} m");
            banded += usize::from(expected == id);
        }
        banded
    };
    // The skirt's hem band (between its rings at 0.72 and 0.66 m), 12 quads; the hair's band below 1.58 m.
    assert_eq!(ids("Cloth", 0.72 - 1e-4, 1), 24);
    assert!(ids("Hair", 1.58, 2) > 0);
    assert_eq!(ids("Skin", f32::NEG_INFINITY, 1), 0);
    assert_eq!(ids("Face", f32::NEG_INFINITY, 1), 0);
}

#[test]
fn outline_normals_are_the_authored_ones() {
    let mesh = character();
    assert!(mesh.authored_outline.iter().all(|&authored| authored), "every part has `_OUTLINE_NORMAL`");
    for vertex in &mesh.vertices {
        assert!((dot(vertex.outline_normal, vertex.outline_normal) - 1.0).abs() < 1e-4);
    }

    // The face and the back of the head: transferred from a sphere on them, so radial from its center.
    let head = [0.0, 1.56, 0.0];
    let radial =
        |position: [f32; 3]| unit([position[0] - head[0], position[1] - head[1], position[2] - head[2]]);
    let face = vertices(&triangles(&mesh, "Face"));
    for &vertex in &face {
        let vertex = &mesh.vertices[vertex];
        assert!(dot(vertex.outline_normal, radial(vertex.position)) > 0.999, "{:?}", vertex.position);
    }

    // The hair: transferred from an ellipsoid inside the cap, so near its outward direction there, and unlike
    // the cap's flat shading normals.
    let (center, radii) = ([0.0, 1.58, -0.01], [0.1, 0.09, 0.11]);
    let mut unlike_shading = 0;
    for vertex in vertices(&triangles(&mesh, "Hair")) {
        let vertex = &mesh.vertices[vertex];
        let gradient = unit(std::array::from_fn(|axis| {
            (vertex.position[axis] - center[axis]) / (radii[axis] * radii[axis])
        }));
        assert!(dot(vertex.outline_normal, gradient) > 0.95, "{:?}", vertex.position);
        unlike_shading += usize::from(dot(vertex.outline_normal, vertex.normal) < 0.99);
    }
    assert!(unlike_shading > 0, "the hair's outline normals are its shading normals");

    // The body's own, welded: every vertex at one position has one outline normal, and smooth-shaded it's
    // near the shading normal.
    for vertex in vertices(&triangles(&mesh, "Cloth")) {
        let vertex = &mesh.vertices[vertex];
        assert!(dot(vertex.outline_normal, vertex.normal) > 0.99, "{:?}", vertex.position);
    }
    let body: Vec<usize> = vertices(&triangles(&mesh, "Cloth"))
        .into_iter()
        .chain(vertices(&triangles(&mesh, "Skin")).into_iter().filter(|vertex| !face.contains(vertex)))
        .collect();
    for &a in &body {
        for &b in &body {
            let (a, b) = (&mesh.vertices[a], &mesh.vertices[b]);
            if a.position == b.position {
                assert!(close(&a.outline_normal, &b.outline_normal), "{:?}", a.position);
            }
        }
    }
}

#[test]
fn the_skin_morph_and_uvs_survive() {
    let mesh = character();
    let skin = mesh.skin.as_ref().expect("the character is skinned");
    let names: BTreeSet<&str> = skin.joints.iter().map(|joint| joint.name.as_str()).collect();
    let bones = [
        "hips",
        "spine",
        "chest",
        "neck",
        "head",
        "upperarm.l",
        "lowerarm.l",
        "hand.l",
        "upperarm.r",
        "lowerarm.r",
        "hand.r",
        "upperleg.l",
        "lowerleg.l",
        "foot.l",
        "upperleg.r",
        "lowerleg.r",
        "foot.r",
    ];
    // The bones, and the armature's node above them.
    assert_eq!(names, bones.iter().copied().chain(["Armature"]).collect());

    // The face, the head and the hair are rigid to `head`; the limbs blend across their joints.
    let joint_of = |bind: u16| skin.joints[usize::from(skin.binds[usize::from(bind)].joint)].name.as_str();
    for slot in ["Face", "Hair"] {
        for vertex in vertices(&triangles(&mesh, slot)) {
            let weights = &skin.weights[vertex];
            assert_eq!((joint_of(weights.binds[0]), weights.weights[0]), ("head", 1.0));
        }
    }
    let blended = skin.weights.iter().filter(|weights| weights.weights[1] > 0.0).count();
    assert!(blended > 0, "no vertex blends two bones");

    // `smile` raises the mouth by 5 mm, and moves nothing but the face.
    assert_eq!(mesh.morph_targets.len(), 1);
    let smile = &mesh.morph_targets[0];
    assert_eq!((smile.name.as_str(), smile.default_weight), ("smile", 0.0));
    let face = vertices(&triangles(&mesh, "Face"));
    let moved: Vec<_> = smile.deltas.iter().filter(|delta| delta.position != [0.0; 3]).collect();
    assert!(!moved.is_empty());
    for delta in moved {
        assert!(face.contains(&(delta.vertex as usize)));
        assert!(close(&delta.position, &[0.0, 0.005, 0.0]), "{:?}", delta.position);
    }

    // The face's UVs project it from the front, mirrored about u = 0.5 (glTF's v runs down).
    for vertex in face {
        let vertex = &mesh.vertices[vertex];
        let [x, y, _] = vertex.position;
        let expected = [0.5 - 0.5 * x / 0.11, 0.5 - 0.5 * (y - 1.56) / 0.11];
        assert!(
            vertex.uv.iter().zip(expected).all(|(uv, expected)| (uv - expected).abs() < 1e-5),
            "{:?}",
            vertex.uv
        );
    }

    // The export holds the character alone: its hull preview, on while it exported, isn't in it.
    let triangles: u32 = mesh.lods[0].submeshes.iter().map(|submesh| submesh.index_count / 3).sum();
    assert_eq!(triangles, 1036);
}

#[test]
fn check_judges_it_at_the_hero_role() {
    let report = check::check(&folder()).unwrap();
    assert!(report.passes(), "{report}");
    assert_eq!(report.meshes.len(), 1, "{report}");
    let judged = &report.meshes[0];
    assert_eq!((judged.source.as_str(), judged.role), ("test_character.glb", Role::Hero));
    for measured in &judged.cooks {
        assert!(
            measured.triangles <= measured.limits.triangles && measured.slots == 4 && measured.joints == 18
        );
    }
    // Its only findings: the four slots draw the materials their extras made, as the export means them to.
    let findings: Vec<(Rule, Severity)> =
        report.findings.iter().map(|finding| (finding.rule, finding.severity)).collect();
    assert_eq!(findings, [(Rule::UnboundSlot, Severity::Warning); 4], "{report}");
}
