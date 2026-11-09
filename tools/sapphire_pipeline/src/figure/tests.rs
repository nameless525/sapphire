use std::path::PathBuf;

use glam::Vec3;

use super::build::build;
use super::*;
use crate::import::gltf;
use crate::test_util::{TempDir, assert_glb_matches, assert_png_matches};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn specs_dir() -> PathBuf {
    repo().join("tools/figures")
}

/// A figure small enough to read: a spine of three joints and two arms, an upright body, a left arm
/// mirrored to the right, a hollow skirt's front and a plate.
fn small() -> Figure {
    sapphire_serialize::from_ron(
        r##"(type: "sapphire::pipeline::Figure", version: 1, data: (
            dir: "test", name: "small", max_triangles: 2000,
            joints: [
                (name: "root", at: (0.0, 0.0, 0.0), aim: Some((0.0, 1.0, 0.0))),
                (name: "hips", parent: Some("root"), at: (0.0, 1.0, 0.0)),
                (name: "chest", parent: Some("hips"), at: (0.0, 1.4, 0.0)),
                (name: "head", parent: Some("chest"), at: (0.0, 1.7, 0.0), aim: Some((0.0, 2.0, 0.0))),
                (name: "upperarm.l", parent: Some("chest"), at: (0.2, 1.6, 0.0)),
                (name: "lowerarm.l", parent: Some("upperarm.l"), at: (0.5, 1.6, 0.0), aim: Some((0.8, 1.6, 0.0))),
                (name: "upperarm.r", parent: Some("chest"), at: (-0.2, 1.6, 0.0)),
                (name: "lowerarm.r", parent: Some("upperarm.r"), at: (-0.5, 1.6, 0.0), aim: Some((-0.8, 1.6, 0.0))),
                (name: "upperleg.l", parent: Some("hips"), at: (0.1, 1.0, 0.0), aim: Some((0.1, 0.0, 0.0))),
            ],
            regions: [
                (name: "cloth", albedo: [(0.0, "#203040"), (1.0, "#101820")]),
                (name: "glow", albedo: [(0.0, "#80ffe0")], emission: [(0.0, 0.0), (0.5, 1.0)]),
            ],
            shapes: [
                Loft((region: "cloth", bones: ["hips", "chest"], from: (0.0, 1.0, 0.0), to: (0.0, 1.6, 0.0),
                      rings: [(at: 0.0, radius: (0.15, 0.1)), (at: 0.5, radius: (0.12, 0.08)), (at: 1.0, radius: (0.16, 0.1))])),
                Loft((region: "cloth", bones: ["upperarm.l", "lowerarm.l"], from: (0.2, 1.6, 0.0), to: (0.8, 1.6, 0.0),
                      rings: [(at: 0.0, radius: (0.05, 0.05)), (at: 0.25, radius: (0.05, 0.05)), (at: 1.0, radius: (0.04, 0.04))], sides: 8, mirror: true)),
                Loft((region: "glow", bones: ["hips", "upperleg.l"], from: (0.0, 1.0, 0.0), to: (0.0, 0.5, 0.0),
                      rings: [(at: 0.0, radius: (0.15, 0.11)), (at: 1.0, radius: (0.25, 0.2))],
                      arc: Some((30.0, 150.0)), thickness: Some(0.01), sides: 6)),
                Slab((region: "cloth", bones: ["head"], origin: (0.0, 1.85, 0.1), u: (1.0, 0.0, 0.0), w: (0.0, 1.0, 0.0),
                      outline: [(-0.08, -0.1), (0.08, -0.1), (0.1, 0.08), (0.0, 0.12), (-0.1, 0.08)], thickness: 0.02)),
            ],
        ))"##,
    )
    .expect("the test spec reads")
}

#[test]
fn a_loft_faces_outward_and_its_mirror_across_x() {
    let built = build(&small()).unwrap();
    let geometry = &built.geometry;
    let set = &geometry.influences[0];
    let (upper_left, upper_right) =
        (built.rig.find("upperarm.l").unwrap(), built.rig.find("upperarm.r").unwrap());
    let (mut left, mut right) = (0, 0);
    for (index, (&position, &normal)) in geometry.positions.iter().zip(&geometry.normals).enumerate() {
        let (position, normal) = (Vec3::from(position), Vec3::from(normal));
        assert!((normal.length() - 1.0).abs() < 1e-4, "vertex {index}'s normal isn't unit");
        let weights: f32 = set.weights[index].iter().sum();
        assert!((weights - 1.0).abs() < 1e-5, "vertex {index}'s weights sum to {weights}");
        // The arms' lofts run along X at y 1.6: their sides face away from that line.
        if (0.25..0.75).contains(&position.x.abs()) && (position.y - 1.6).abs() < 0.06 {
            let off = Vec3::new(0.0, position.y - 1.6, position.z);
            if off.length() > 0.03 && normal.x.abs() < 0.5 {
                assert!(normal.dot(off) > 0.0, "vertex {index} at {position} faces in");
            }
            let joint = set.joints[index][0] as usize;
            if position.x > 0.0 && position.x < 0.4 {
                assert_eq!(joint, upper_left, "vertex {index} at {position}");
                left += 1;
            }
            if position.x < 0.0 && position.x > -0.4 {
                assert_eq!(joint, upper_right, "vertex {index} at {position}");
                right += 1;
            }
        }
    }
    assert!(left > 0 && left == right, "{left} on the left, {right} on the right");
    // Every face winds as its normals point.
    for triangle in geometry.indices.as_chunks::<3>().0 {
        let [a, b, c] = triangle.map(|corner| Vec3::from(geometry.positions[corner as usize]));
        let face = (b - a).cross(c - a);
        if face.length() < 1e-9 {
            continue;
        }
        let normals: Vec3 = triangle.iter().map(|&index| Vec3::from(geometry.normals[index as usize])).sum();
        assert!(face.dot(normals) > 0.0, "a face winds against its normals at {a}");
    }
}

#[test]
fn its_uvs_lie_in_their_regions_cells() {
    let figure = small();
    let built = build(&figure).unwrap();
    for uv in &built.geometry.uvs {
        let cell = (uv[0] * figure.regions.len() as f32).floor();
        assert!(cell == 0.0 || cell == 1.0, "{uv:?}");
        assert!((uv[0] * 2.0).fract() == 0.5, "{uv:?} isn't at a cell's middle");
        assert!((0.0..=1.0).contains(&uv[1]), "{uv:?}");
    }
}

/// What the importer reads back is the figure's skeleton at the spec's places, every vertex skinned.
#[test]
fn the_model_imports_with_its_skeleton_at_rest() {
    let figure = small();
    let built = build(&figure).unwrap();
    let dir = TempDir::new("figure_import");
    let path = dir.path().join("small.glb");
    fs::write(&path, built.to_glb()).unwrap();
    let mesh = gltf::import(&path).expect("it imports");
    let skin = mesh.skin.as_ref().expect("skinned");
    // The importer takes the joints' ancestors too: the model's top node, as KayKit's `Rig_Medium`.
    let names: Vec<&str> = skin.joints.iter().map(|joint| joint.name.as_str()).collect();
    let spec: Vec<&str> = figure.joints.iter().map(|joint| joint.name.as_str()).collect();
    assert_eq!(names[0], "rig");
    assert_eq!(names[1..], spec);
    assert_eq!(skin.weights.len(), mesh.vertices.len());
    assert_eq!(mesh.indices.len(), built.geometry.indices.len());
    // A joint's inverse bind takes its rest place to the origin.
    assert_eq!(skin.binds.len(), figure.joints.len());
    for bind in &skin.binds {
        let name = &skin.joints[usize::from(bind.joint)].name;
        let joint = figure.joints.iter().find(|joint| &joint.name == name).expect("a spec joint");
        let inverse = glam::Mat4::from_cols_array_2d(&bind.inverse_bind_matrix);
        let origin = inverse.transform_point3(Vec3::from(joint.at));
        assert!(origin.length() < 1e-5, "{name}: {origin}");
    }
    let names: Vec<&str> = mesh.materials.iter().map(|slot| slot.name.as_str()).collect();
    assert_eq!(names, ["small"]);
}

#[test]
fn a_bad_spec_says_what_is_wrong() {
    let mut figure = small();
    if let Shape::Loft(loft) = &mut figure.shapes[0] {
        loft.bones.push("tail".to_owned());
    }
    let error = build(&figure).unwrap_err().to_string();
    assert!(error.contains("shape 0") && error.contains("\"tail\""), "{error}");

    let mut figure = small();
    if let Shape::Loft(loft) = &mut figure.shapes[2] {
        loft.thickness = None;
    }
    let error = build(&figure).unwrap_err().to_string();
    assert!(error.contains("shape 2") && error.contains("thickness"), "{error}");

    let mut figure = small();
    figure.max_triangles = 10;
    let error = build(&figure).unwrap_err().to_string();
    assert!(error.contains("budget of 10"), "{error}");

    let mut figure = small();
    figure.joints[2].parent = Some("head".to_owned());
    let error = build(&figure).unwrap_err().to_string();
    assert!(error.contains("\"chest\"") && error.contains("before"), "{error}");

    let mut figure = small();
    figure.regions = (0..7).map(|_| figure.regions[0].clone()).collect();
    let error = build(&figure).unwrap_err().to_string();
    assert!(error.contains("regions"), "{error}");

    let mut figure = small();
    figure.regions[0].albedo[0].1 = "teal".to_owned();
    let error = paint::palette_png(&figure).unwrap_err().to_string();
    assert!(error.contains("\"cloth\"") && error.contains("#rrggbb"), "{error}");
}

#[test]
fn the_control_map_holds_each_region_s_id_and_the_mask_its_glow() {
    use sapphire_assets::character_maps::ilm::decode_material_id;
    let figure = small();
    let control = image::load_from_memory(&paint::control_png(&figure).unwrap()).unwrap().to_rgba8();
    let layers = image::load_from_memory(&paint::layers_png(&figure).unwrap()).unwrap().to_rgba8();
    let (width, height) = control.dimensions();
    for (index, _) in figure.regions.iter().enumerate() {
        let x = (index as u32 * 2 + 1) * width / 4;
        let id = decode_material_id(f32::from(control.get_pixel(x, height / 2).0[3]) / 255.0);
        assert_eq!(usize::from(id.get()), index);
    }
    // The glow's emission ramps up down its cell; the cloth has none.
    let glow = |y: u32| layers.get_pixel(3 * width / 4, y).0[2];
    assert!(glow(0) < 4, "{}", glow(0));
    assert_eq!(glow(height - 1), 255);
    assert_eq!(layers.get_pixel(width / 4, height - 1).0[2], 0);
}

/// Every figure has KayKit's 23 joints by their names and parents, so the pack's clips retarget onto
/// it by sidecar and what finds the warrior's joints by name finds its.
#[test]
fn every_figure_has_kaykit_s_joints() {
    const KAYKIT: [(&str, Option<&str>); 23] = [
        ("root", None),
        ("hips", Some("root")),
        ("spine", Some("hips")),
        ("chest", Some("spine")),
        ("head", Some("chest")),
        ("upperarm.l", Some("chest")),
        ("lowerarm.l", Some("upperarm.l")),
        ("wrist.l", Some("lowerarm.l")),
        ("hand.l", Some("wrist.l")),
        ("handslot.l", Some("hand.l")),
        ("upperarm.r", Some("chest")),
        ("lowerarm.r", Some("upperarm.r")),
        ("wrist.r", Some("lowerarm.r")),
        ("hand.r", Some("wrist.r")),
        ("handslot.r", Some("hand.r")),
        ("upperleg.l", Some("hips")),
        ("lowerleg.l", Some("upperleg.l")),
        ("foot.l", Some("lowerleg.l")),
        ("toes.l", Some("foot.l")),
        ("upperleg.r", Some("hips")),
        ("lowerleg.r", Some("upperleg.r")),
        ("foot.r", Some("lowerleg.r")),
        ("toes.r", Some("foot.r")),
    ];
    let paths = specs(&specs_dir()).unwrap();
    assert!(!paths.is_empty(), "no specs in tools/figures");
    for path in paths {
        let figure = read(&path).unwrap();
        for (name, parent) in KAYKIT {
            let joint = figure.joints.iter().find(|joint| joint.name == name);
            let joint = joint.unwrap_or_else(|| panic!("{}: no joint {name:?}", path.display()));
            assert_eq!(joint.parent.as_deref(), parent, "{}: {name}'s parent", path.display());
        }
        assert_eq!(figure.joints.len(), KAYKIT.len(), "{}: joints beyond KayKit's", path.display());
    }
}

#[test]
fn generating_twice_is_byte_identical() {
    for path in specs(&specs_dir()).unwrap() {
        let figure = read(&path).unwrap();
        assert_eq!(files(&figure).unwrap(), files(&figure).unwrap(), "{}", path.display());
    }
}

/// The committed models and maps are what their specs make, byte for byte, on every target and OS:
/// rings are placed by `det::sin_cos`, joint rotations come from `det::quat_from_mat3`, and colours
/// go through `det::pow`, so nothing reaches a platform's C library or glam's SIMD backends. A failure
/// on one OS is a determinism bug, never bytes to commit per platform: the comparison by value says
/// how far off it is first. Regenerate with `cargo run -p sapphire_pipeline --profile tools --
/// gen-figures --specs tools/figures --out content --force`.
#[test]
fn the_committed_figures_are_generated() {
    let content = repo().join("content");
    for path in specs(&specs_dir()).unwrap() {
        let figure = read(&path).unwrap();
        for (name, generated) in files(&figure).unwrap() {
            let committed = fs::read(content.join(&name))
                .unwrap_or_else(|error| panic!("{name}: {error}; regenerate it with gen-figures"));
            if committed == generated {
                continue;
            }
            if name.ends_with(".glb") {
                assert_glb_matches(&name, &committed, &generated, |_, a, b| a == b);
            } else {
                assert_png_matches(&name, &committed, &generated);
            }
            panic!("{name}: the generated bytes differ from the committed ones");
        }
    }
}

/// Every committed figure cooks under its spec's budget at LOD0 in both cooks, by its sidecar, and
/// each coarser level has fewer triangles than the one before.
#[test]
fn every_figure_cooks_under_its_budget() {
    use crate::cook::{Cook, cook_mesh};
    let content = repo().join("content");
    for path in specs(&specs_dir()).unwrap() {
        let figure = read(&path).unwrap();
        let model = content.join(&figure.dir).join(format!("{}.glb", figure.name));
        let meta = crate::meta::read(&crate::meta::sidecar_path(&model)).unwrap();
        let settings = meta.importer.mesh().expect("a mesh's sidecar").clone();
        for cook in Cook::ALL {
            let mesh = cook_mesh(&model, &settings, cook).unwrap();
            let triangles: Vec<u32> = mesh
                .lods
                .iter()
                .map(|lod| lod.submeshes.iter().map(|submesh| submesh.index_count / 3).sum())
                .collect();
            assert!(triangles[0] <= figure.max_triangles, "{} {cook}: {triangles:?}", figure.name);
            assert!(
                triangles.windows(2).all(|pair| pair[1] < pair[0]),
                "{} {cook}: {triangles:?}",
                figure.name
            );
        }
    }
}
