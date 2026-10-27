use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use glam::{Mat4, Quat, Vec2, Vec3};
use sapphire_assets::mesh::{Bounds, PositionFormat};

use super::*;
use crate::import::gltf;

/// A fixture `tools/blender/fbx_fixtures.py` made, by file name.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/fbx").join(name)
}

/// VVayToyek's pavilion (CC0, `content/third_party/`), a real file from Blender 3.2.
fn pavilion() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/third_party/vvaytoyek_chinese_pavilion/chinese_four_corner_pavilion.fbx")
}

/// Packs and reads back `mesh`, so every structural invariant of the cooked format is checked.
fn assert_cooks(mesh: &ImportedMesh) {
    let bytes =
        mesh.pack(PositionFormat::Float32, &BTreeMap::new()).expect("pack").to_cooked().expect("cook");
    sapphire_assets::mesh::Mesh::read(&bytes).expect("a valid cooked mesh");
}

/// One triangle corner, with everything a vertex holds in world terms: joints by name, and the
/// position the skin puts it at in the rest pose.
#[derive(Clone, Debug)]
struct Corner {
    position: Vec3,
    normal: Vec3,
    uv: Vec2,
    color: [u8; 3],
    /// Where the rest pose's skinning matrices put it; its position when unskinned.
    skinned: Vec3,
    /// (joint name, weight), by name.
    influences: Vec<(String, f32)>,
    /// Per morph target, by name: its position and normal deltas.
    morphs: Vec<(Vec3, Vec3)>,
    /// Whether every vertex at its position shares its normal: it lies on no hard edge.
    smooth: bool,
}

/// Each joint's model-space rest transform.
fn rest_world(skin: &ImportedSkin) -> Vec<Mat4> {
    let mut world: Vec<Mat4> = Vec::with_capacity(skin.joints.len());
    for joint in &skin.joints {
        let JointTransform { translation, rotation, scale } = joint.rest;
        let local = Mat4::from_scale_rotation_translation(
            Vec3::from(scale),
            Quat::from_array(rotation),
            Vec3::from(translation),
        );
        world.push(joint.parent.map_or(local, |parent| world[usize::from(parent)] * local));
    }
    world
}

/// Every triangle of every slot, by slot name, with its corners in file order.
fn triangles(mesh: &ImportedMesh) -> BTreeMap<String, Vec<[Corner; 3]>> {
    let skinning: Vec<Mat4> = mesh.skin.as_ref().map_or_else(Vec::new, |skin| {
        let world = rest_world(skin);
        skin.binds
            .iter()
            .map(|bind| world[usize::from(bind.joint)] * Mat4::from_cols_array_2d(&bind.inverse_bind_matrix))
            .collect()
    });
    let mut targets: Vec<&ImportedMorphTarget> = mesh.morph_targets.iter().collect();
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    let deltas: Vec<HashMap<u32, (Vec3, Vec3)>> = targets
        .iter()
        .map(|target| {
            target.deltas.iter().map(|d| (d.vertex, (Vec3::from(d.position), Vec3::from(d.normal)))).collect()
        })
        .collect();
    let mut normals_at: HashMap<[u32; 3], Vec<Vec3>> = HashMap::new();
    for vertex in &mesh.vertices {
        normals_at.entry(vertex.position.map(f32::to_bits)).or_default().push(Vec3::from(vertex.normal));
    }
    let corner = |index: u32| {
        let vertex = &mesh.vertices[index as usize];
        let position = Vec3::from(vertex.position);
        let (skinned, influences) = match &mesh.skin {
            Some(skin) => {
                let weights = skin.weights[index as usize];
                let skinned = weights
                    .binds
                    .iter()
                    .zip(weights.weights)
                    .map(|(&bind, weight)| skinning[usize::from(bind)].transform_point3(position) * weight)
                    .sum();
                let mut influences: Vec<(String, f32)> = weights
                    .binds
                    .iter()
                    .zip(weights.weights)
                    .filter(|&(_, weight)| weight > 0.0)
                    .map(|(&bind, weight)| {
                        let joint = skin.binds[usize::from(bind)].joint;
                        (skin.joints[usize::from(joint)].name.clone(), weight)
                    })
                    .collect();
                influences.sort_by(|a, b| a.0.cmp(&b.0));
                (skinned, influences)
            }
            None => (position, Vec::new()),
        };
        Corner {
            position,
            normal: Vec3::from(vertex.normal),
            uv: Vec2::from(vertex.uv),
            color: vertex.color,
            skinned,
            influences,
            morphs: deltas.iter().map(|deltas| deltas.get(&index).copied().unwrap_or_default()).collect(),
            smooth: normals_at[&vertex.position.map(f32::to_bits)]
                .iter()
                .all(|normal| normal.abs_diff_eq(Vec3::from(vertex.normal), 1e-3)),
        }
    };
    let mut slots: BTreeMap<String, Vec<[Corner; 3]>> = BTreeMap::new();
    for submesh in &mesh.lods[0].submeshes {
        let name = mesh.materials[submesh.material as usize].name.clone();
        let range = submesh.first_index as usize..(submesh.first_index + submesh.index_count) as usize;
        let list = slots.entry(name).or_default();
        for &[a, b, c] in mesh.indices[range].as_chunks::<3>().0 {
            list.push([corner(a), corner(b), corner(c)]);
        }
    }
    slots
}

/// How far apart two corners are, the worst of their attributes in units of each one's
/// tolerance; under 1 is a match.
///
/// Normals get 1e-4 a component: Blender's glTF exporter rounds them to four decimals (and its
/// morph normals before subtracting), a hundredth of a degree, under the cooked encoding's
/// precision. Colors may differ by one step: glTF stores them in 16 bits. Morph normal deltas
/// are compared away from hard edges only: FBX blend shapes offset each vertex's normal, so
/// Blender writes the change of its vertex normal, which is the change of its corners' normals
/// only where they are all one.
fn distance(a: &Corner, b: &Corner) -> f32 {
    let mut worst = (a.position - b.position).abs().max_element() / 2e-5;
    worst = worst.max((a.normal - b.normal).abs().max_element() / 1e-4);
    worst = worst.max((a.uv - b.uv).abs().max_element() / 1e-6);
    let color = a.color.iter().zip(b.color).map(|(&x, y)| x.abs_diff(y)).max().unwrap_or(0);
    worst = worst.max(f32::from(color) / 1.5);
    worst = worst.max((a.skinned - b.skinned).abs().max_element() / 2e-5);
    if a.influences.len() != b.influences.len()
        || a.influences.iter().zip(&b.influences).any(|(x, y)| x.0 != y.0)
    {
        return f32::INFINITY;
    }
    for (x, y) in a.influences.iter().zip(&b.influences) {
        worst = worst.max((x.1 - y.1).abs() / 1e-4);
    }
    for (x, y) in a.morphs.iter().zip(&b.morphs) {
        worst = worst.max((x.0 - y.0).abs().max_element() / 2e-5);
        if a.smooth && b.smooth {
            worst = worst.max((x.1 - y.1).abs().max_element() / 2e-4);
        }
    }
    worst
}

/// The corners of `b` matched to `a`'s, keeping the winding: `b` rotated so its corners pair
/// with `a`'s in order, or `None` when no rotation does.
fn triangle_distance(a: &[Corner; 3], b: &[Corner; 3]) -> f32 {
    (0..3)
        .map(|shift| (0..3).map(|k| distance(&a[k], &b[(k + shift) % 3])).fold(0.0, f32::max))
        .fold(f32::INFINITY, f32::min)
}

/// Asserts the two imports hold the same triangles in the same slots, in any order, each within
/// its attributes' tolerances.
fn assert_same_triangles(fbx: &ImportedMesh, gltf: &ImportedMesh) {
    let (fbx_slots, gltf_slots) = (triangles(fbx), triangles(gltf));
    assert_eq!(fbx_slots.keys().collect::<Vec<_>>(), gltf_slots.keys().collect::<Vec<_>>(), "slots");
    for (slot, fbx_triangles) in &fbx_slots {
        let mut unmatched = gltf_slots[slot].clone();
        assert_eq!(fbx_triangles.len(), unmatched.len(), "{slot}: triangle count");
        for triangle in fbx_triangles {
            let (best, gap) = unmatched
                .iter()
                .enumerate()
                .map(|(index, other)| (index, triangle_distance(triangle, other)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .expect("as many left as there are triangles to match");
            assert!(
                gap < 1.0,
                "{slot}: no glTF triangle matches {triangle:?}; the nearest is {:?}",
                unmatched[best]
            );
            unmatched.swap_remove(best);
        }
    }
}

fn import_both(name: &str) -> (ImportedMesh, ImportedMesh) {
    let fbx = import(&fixture(&format!("{name}.fbx"))).expect("import the FBX");
    let gltf = gltf::import(&fixture(&format!("{name}.glb"))).expect("import the glTF");
    assert_cooks(&fbx);
    (fbx, gltf)
}

#[test]
fn a_static_scene_imports_as_its_gltf_does() {
    let (fbx, gltf) = import_both("static_multi");
    assert_same_triangles(&fbx, &gltf);
    // Corners merge into the same vertices, so the same vertex count.
    assert_eq!(fbx.vertices.len(), gltf.vertices.len());
    assert_eq!(fbx.lods[0].submeshes.len(), gltf.lods[0].submeshes.len());
    let materials = |mesh: &ImportedMesh| -> BTreeMap<String, AnimeCharacterMaterial> {
        mesh.materials.iter().map(|m| (m.name.clone(), m.generated.clone())).collect()
    };
    let (fbx_materials, gltf_materials) = (materials(&fbx), materials(&gltf));
    assert_eq!(fbx_materials.len(), 3);
    for (name, material) in &fbx_materials {
        let other = &gltf_materials[name];
        for (a, b) in material.base_color.iter().zip(other.base_color) {
            assert!((a - b).abs() < 1e-6, "{name}: {:?} vs {:?}", material.base_color, other.base_color);
        }
        let other = AnimeCharacterMaterial { base_color: material.base_color, ..other.clone() };
        assert_eq!(*material, other, "{name}");
    }
    assert!(fbx.skin.is_none() && fbx.morph_targets.is_empty());
    assert_eq!(fbx.authored_outline, vec![false; fbx.vertices.len()]);
}

#[test]
fn a_skinned_scene_with_blend_shapes_imports_as_its_gltf_does() {
    let (fbx, gltf) = import_both("skinned_morph");
    assert_same_triangles(&fbx, &gltf);
    assert_eq!(fbx.vertices.len(), gltf.vertices.len());

    // The same joints, in the same order under the same parents, standing in the same places.
    let (fbx_skin, gltf_skin) = (fbx.skin.as_ref().unwrap(), gltf.skin.as_ref().unwrap());
    let joints = |skin: &ImportedSkin| -> Vec<(String, Option<u16>)> {
        skin.joints.iter().map(|joint| (joint.name.clone(), joint.parent)).collect()
    };
    assert_eq!(joints(fbx_skin), joints(gltf_skin));
    for ((joint, a), b) in fbx_skin.joints.iter().zip(rest_world(fbx_skin)).zip(rest_world(gltf_skin)) {
        let (a, b) = (a.w_axis.truncate(), b.w_axis.truncate());
        assert!(a.abs_diff_eq(b, 1e-5), "{}: {a} vs {b}", joint.name);
    }
    let targets = |mesh: &ImportedMesh| -> Vec<(String, f32)> {
        let mut targets: Vec<_> =
            mesh.morph_targets.iter().map(|t| (t.name.clone(), t.default_weight)).collect();
        targets.sort_by(|a, b| a.0.cmp(&b.0));
        targets
    };
    assert_eq!(
        targets(&fbx),
        [("Bulge".to_owned(), 0.0), ("Lean".to_owned(), 0.0), ("Rise".to_owned(), 0.25)]
    );
    assert_eq!(targets(&fbx), targets(&gltf));
    // The bulge turns the normals of corners away from the hard edges, which were compared.
    let turned = triangles(&fbx)
        .values()
        .flatten()
        .flatten()
        .filter(|corner| corner.smooth && corner.morphs[0].1.length() > 0.01)
        .count();
    assert!(turned > 0);
}

#[test]
fn the_pavilion_imports_in_meters_standing_on_its_origin() {
    let mesh = import(&pavilion()).expect("import");
    assert_cooks(&mesh);
    let triangles = mesh.indices.len() / 3;
    assert_eq!(triangles, 76_926);
    assert_eq!(mesh.materials.len(), 22);
    assert!(mesh.materials.iter().any(|slot| slot.name == "瓦面"), "the tile surface's slot");
    let bounds = Bounds::enclosing(mesh.vertices.iter().map(|v| v.position));
    // 35.7 m to the finial in the file's own units, 38.9 m square, on the ground at its origin.
    assert!(bounds.min[1].abs() < 1e-4 && (bounds.max[1] - 35.69).abs() < 0.01, "{bounds:?}");
    assert!((bounds.max[0] - 19.433).abs() < 0.01 && (bounds.min[2] + 19.433).abs() < 0.01, "{bounds:?}");
    // Triangles face the way their corners' normals do: the winding survived triangulation and
    // the conversion. All but 40 do; those are slivers folded over in the source.
    let facing = (0..triangles)
        .filter(|&t| {
            let [a, b, c] = [0, 1, 2].map(|k| &mesh.vertices[mesh.indices[3 * t + k] as usize]);
            let face = (Vec3::from(b.position) - Vec3::from(a.position))
                .cross(Vec3::from(c.position) - Vec3::from(a.position));
            let normals = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            face.dot(normals) > 0.0
        })
        .count();
    assert!(facing as f64 > 0.999 * triangles as f64, "{facing} of {triangles}");
}

#[test]
fn the_importer_names_the_locked_ufbx_version() {
    let lock = include_str!("../../../../../Cargo.lock");
    let version = lock
        .split("[[package]]")
        .find(|package| package.contains("\nname = \"ufbx\"\n"))
        .and_then(|package| package.split("\nversion = \"").nth(1))
        .and_then(|rest| rest.split('"').next())
        .expect("ufbx in Cargo.lock");
    assert_eq!(
        IMPORTER.name,
        format!("fbx (ufbx {version})"),
        "a ufbx upgrade can change imports: rename the importer"
    );
}

/// A minimal ASCII FBX 7.4 document: `axes` are `GlobalSettings`' up, front and coord axes, each
/// with its sign, `unit_cm` its centimeters a unit, `objects` and `connections` the scene.
fn ascii_fbx(axes: [i32; 6], unit_cm: f64, objects: &str, connections: &str) -> Vec<u8> {
    let names = ["UpAxis", "UpAxisSign", "FrontAxis", "FrontAxisSign", "CoordAxis", "CoordAxisSign"];
    let mut properties: String = names
        .iter()
        .zip(axes)
        .map(|(name, value)| format!("\t\tP: \"{name}\", \"int\", \"Integer\", \"\",{value}\n"))
        .collect();
    properties += &format!("\t\tP: \"UnitScaleFactor\", \"double\", \"Number\", \"\",{unit_cm}\n");
    format!(
        "; FBX 7.4.0 project file\n\
         FBXHeaderExtension:  {{\n\tFBXHeaderVersion: 1003\n\tFBXVersion: 7400\n}}\n\
         GlobalSettings:  {{\n\tVersion: 1000\n\tProperties70:  {{\n{properties}\t}}\n}}\n\
         Objects:  {{\n{objects}}}\n\
         Connections:  {{\n{connections}}}\n"
    )
    .into_bytes()
}

/// Y up, Z toward the viewer, X right: the engine's own axes.
const Y_UP: [i32; 6] = [1, 1, 2, 1, 0, 1];

/// A mesh node `name` (ID `id`, its geometry `id + 1`) at the scene's root, with `vertices` and
/// `polygons` (FBX's indices: each polygon's last as `-(index + 1)`): its objects and
/// connections.
fn mesh_object(id: u64, name: &str, vertices: &[f64], polygons: &[i32]) -> (String, String) {
    let join = |values: Vec<String>| values.join(",");
    let objects = format!(
        "\tGeometry: {}, \"Geometry::{name}\", \"Mesh\" {{\n\
         \t\tVertices: *{} {{ a: {} }}\n\
         \t\tPolygonVertexIndex: *{} {{ a: {} }}\n\
         \t}}\n\
         \tModel: {id}, \"Model::{name}\", \"Mesh\" {{\n\t\tVersion: 232\n\t}}\n",
        id + 1,
        vertices.len(),
        join(vertices.iter().map(f64::to_string).collect()),
        polygons.len(),
        join(polygons.iter().map(i32::to_string).collect()),
    );
    let connections = format!("\tC: \"OO\",{},{id}\n\tC: \"OO\",{id},0\n", id + 1);
    (objects, connections)
}

#[test]
fn polygons_are_triangulated_and_faces_without_normals_or_materials_get_defaults() {
    // A concave pentagon (an arrow notched at (1, 1)) and a quad in the XY plane, facing +Z, in
    // meters, with no normals, texture coordinates, colors or materials.
    #[rustfmt::skip]
    let vertices = [
        0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 2.0, 2.0, 0.0, 1.0, 1.0, 0.0, 0.0, 2.0, 0.0,
        3.0, 0.0, 0.0, 4.0, 0.0, 0.0, 4.0, 1.0, 0.0, 3.0, 1.0, 0.0,
    ];
    let (objects, connections) = mesh_object(100, "Plate", &vertices, &[0, 1, 2, 3, -5, 5, 6, 7, -9]);
    let mesh = import_slice(&ascii_fbx(Y_UP, 100.0, &objects, &connections)).expect("import");
    assert_cooks(&mesh);
    assert_eq!(mesh.indices.len(), 3 * (3 + 2));
    let mut area = 0.0;
    for t in 0..5 {
        let [a, b, c] =
            [0, 1, 2].map(|k| Vec3::from(mesh.vertices[mesh.indices[3 * t + k] as usize].position));
        let normal = (b - a).cross(c - a);
        assert!(normal.z > 0.0 && normal.x.abs() + normal.y.abs() < 1e-6, "triangle {t} faces {normal}");
        area += normal.length() / 2.0;
    }
    // The pentagon's 3 m² and the quad's 1 m²: no triangle crosses the notch.
    assert!((area - 4.0).abs() < 1e-5, "{area}");
    for vertex in &mesh.vertices {
        assert_eq!(
            (vertex.normal, vertex.uv, vertex.color),
            ([0.0, 0.0, 1.0], [0.0, 0.0], VERTEX_COLOR_NEUTRAL)
        );
    }
    assert_eq!(mesh.materials.len(), 1);
    assert_eq!(mesh.materials[0].name, DEFAULT_SLOT);
    let default = &mesh.materials[0].generated;
    assert_eq!((default.base_color, default.alpha), ([1.0; 4], AlphaMode::Opaque));
}

#[test]
fn z_up_centimeter_files_convert_to_y_up_meters() {
    // 3ds Max's and Blender's axes: Z up, -Y toward the viewer, X right; a unit a centimeter.
    let z_up = [2, 1, 1, -1, 0, 1];
    let (objects, connections) =
        mesh_object(100, "Corner", &[100.0, 0.0, 0.0, 0.0, -100.0, 0.0, 0.0, 0.0, 100.0], &[0, 1, -3]);
    let mesh = import_slice(&ascii_fbx(z_up, 1.0, &objects, &connections)).expect("import");
    let positions: Vec<Vec3> = mesh.vertices.iter().map(|v| Vec3::from(v.position)).collect();
    // Right stays +X, the viewer's side becomes +Z and up +Y, in meters.
    for (position, expected) in positions.iter().zip([Vec3::X, Vec3::Z, Vec3::Y]) {
        assert!(position.abs_diff_eq(expected, 1e-6), "{positions:?}");
    }
}

#[test]
fn files_that_are_not_fbx_or_hold_no_triangles_are_errors() {
    assert!(matches!(import_slice(b"not an FBX file"), Err(FbxImportError::Parse(_))));
    let empty = ascii_fbx(Y_UP, 100.0, "", "");
    assert!(matches!(import_slice(&empty), Err(FbxImportError::NoGeometry)));
    // A point and a line.
    let (objects, connections) =
        mesh_object(100, "Wire", &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0], &[-1, 1, -3]);
    let wire = ascii_fbx(Y_UP, 100.0, &objects, &connections);
    assert!(matches!(import_slice(&wire), Err(FbxImportError::NoGeometry)));
}
