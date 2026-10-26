//! Procedurally generated source assets for bringing up the renderer.
//!
//! They are written as ordinary sources (glTF, anime map documents and their PNG masks), so
//! they travel the same import → bake → cook path as artist content:
//!
//! - the mannequin, `mannequin.glb`;
//! - the shader swatches' content ([`swatch`]): a ramp, a control map and three materials
//!   for the sandbox's `--swatches` spheres (M1 C1);
//! - the face test head's content ([`face`], in `face/`): a head, its face material, and the
//!   SDF face map (with its threshold masks), face mask and expression map that material reads,
//!   for the sandbox's `--faces` (M1 C3);
//! - the sandbox sky's content ([`sky`]): card cloud, cirrus, moon, star and Milky Way maps and
//!   the sky material that draws them (M1 W2);
//! - the environment's test content ([`environment`], in `environment/`): a courtyard on rolling
//!   ground, its environment materials, a terminator band and a painted ground map, for the
//!   sandbox's `--environment` (M1 W4);
//! - the foliage's test content ([`foliage`], in `foliage/`): a stylized tree, a bush and a
//!   grass patch, their foliage materials and the cards' maps, for the sandbox's `--foliage`
//!   (M1 W6);
//! - the hard-edged outline sample ([`hard_edges`], `hard_edges.glb`): flat-shaded shapes for the
//!   sandbox's `--hard-edges`, whose hulls must stay closed at every corner (M1 C4).

pub mod environment;
pub mod face;
pub mod foliage;
pub mod hard_edges;
mod noise;
mod shapes;
pub mod sky;
pub mod swatch;

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::color::srgb8;
use crate::gltf_writer::GltfWriter;

/// File name of the mannequin sample.
pub const MANNEQUIN_FILE_NAME: &str = "mannequin.glb";

/// `COLOR_0` green on the head: the outline-width scale (255 = 1.0). Faces read better
/// with a finer line than clothing does.
pub const HEAD_OUTLINE_WIDTH: u8 = 140;

/// Writes every sample source asset into `dir` (creating it and the subdirectories they sit
/// in) and returns their paths.
pub fn write_all(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let files = [
        (MANNEQUIN_FILE_NAME, mannequin_glb()),
        (swatch::RAMP_FILE_NAME, swatch::ramp_document().into_bytes()),
        (swatch::ILM_FILE_NAME, swatch::ilm_document().into_bytes()),
        (swatch::MATERIAL_ID_FILE_NAME, swatch::material_id_png()?),
        (swatch::SHADOW_BIAS_FILE_NAME, swatch::shadow_bias_png()?),
        (swatch::RAMP_MATERIAL_FILE_NAME, swatch::ramp_material_document().into_bytes()),
        (swatch::SUBSURFACE_MATERIAL_FILE_NAME, swatch::subsurface_material_document().into_bytes()),
        (swatch::BAND_MATERIAL_FILE_NAME, swatch::band_material_document().into_bytes()),
        (sky::CLOUDS_FILE_NAME, sky::clouds_png()?),
        (sky::CIRRUS_FILE_NAME, sky::cirrus_png()?),
        (sky::MOON_FILE_NAME, sky::moon_png()?),
        (sky::STARS_FILE_NAME, sky::stars_png()?),
        (sky::MILKY_WAY_FILE_NAME, sky::milky_way_png()?),
        (sky::MATERIAL_FILE_NAME, sky::material_document().into_bytes()),
        (hard_edges::FILE_NAME, hard_edges::glb()),
    ]
    .map(|(name, bytes)| (name.to_owned(), bytes));
    let mut paths = Vec::new();
    let nested = face::files()?.into_iter().chain(environment::files()?).chain(foliage::files()?);
    for (name, bytes) in files.into_iter().chain(nested) {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap_or(dir))?;
        std::fs::write(&path, bytes)?;
        paths.push(path);
    }
    Ok(paths)
}

/// A character material in an anime palette. Colors are 8-bit sRGB, the way an artist picks
/// them; they are stored linear in the file.
struct Look {
    name: &'static str,
    base: [u8; 3],
    /// Multiplier on `base` in shadow; tinted cool or warm rather than grey.
    shade: [u8; 3],
    /// Outlines are a dark tint of the surface, not black.
    outline: [u8; 3],
    /// `[shadow threshold, softness, rim strength, outline width in pixels at 1080p]`.
    character_shading: [f32; 4],
    /// The regions' rim scales, `[strength, width, overlap]` (M1 C5), or none for one region
    /// that rims at its defaults.
    rim_regions: &'static [[f32; 3]],
    /// ZZZ's back-light weighting of the rim (`Rim::back_light`, M1 C5): the outfit's rim
    /// strengthens as the light comes round behind the figure.
    back_light: f32,
}

/// The skin's regions: the head and the rest (0), and the arms (1, the arm meshes' vertex
/// material ID), which keep their rim to the figure's outer silhouette, none where an arm crosses
/// the torso (M1 C5).
const SKIN_RIM_REGIONS: [[f32; 3]; 2] = [[1.0, 1.0, 1.0], [1.0, 1.0, 0.0]];
/// The arms' region of the skin material.
const ARM_REGION: f32 = 1.0;

const SKIN: Look = Look {
    name: "Skin",
    base: [255, 224, 206],
    shade: [236, 170, 168],
    outline: [110, 52, 48],
    character_shading: [0.1, 0.03, 0.25, 4.1],
    rim_regions: &SKIN_RIM_REGIONS,
    back_light: 0.0,
};
const OUTFIT: Look = Look {
    name: "Outfit",
    base: [38, 42, 66],
    shade: [150, 140, 200],
    outline: [12, 12, 22],
    character_shading: [0.0, 0.02, 0.4, 5.5],
    rim_regions: &[],
    back_light: 1.0,
};
const HAIR: Look = Look {
    name: "Hair",
    base: [72, 196, 214],
    shade: [110, 120, 210],
    outline: [18, 56, 84],
    character_shading: [0.05, 0.015, 0.5, 4.8],
    rim_regions: &[],
    back_light: 0.0,
};
const ACCENT: Look = Look {
    name: "Accent",
    base: [214, 240, 46],
    shade: [200, 160, 120],
    outline: [64, 60, 10],
    character_shading: [-0.1, 0.01, 0.3, 5.5],
    rim_regions: &[],
    back_light: 0.0,
};
const PEDESTAL: Look = Look {
    name: "Pedestal",
    base: [118, 122, 134],
    shade: [150, 150, 178],
    outline: [26, 26, 32],
    character_shading: [0.2, 0.05, 0.1, 6.9],
    rim_regions: &[],
    back_light: 0.0,
};

/// Every material in the mannequin, in file order.
const LOOKS: [&Look; 5] = [&SKIN, &OUTFIT, &HAIR, &ACCENT, &PEDESTAL];

impl Look {
    fn material(&self) -> Value {
        let [r, g, b] = self.base;
        let mut material = json!({
            "name": self.name,
            "pbrMetallicRoughness": {
                "baseColorFactor": srgb8(r, g, b),
                "metallicFactor": 0.0,
                "roughnessFactor": 1.0,
            },
            "extras": {
                "shade_color": srgb8(self.shade[0], self.shade[1], self.shade[2]),
                "outline_color": srgb8(self.outline[0], self.outline[1], self.outline[2]),
                "character_shading": self.character_shading,
            },
        });
        if !self.rim_regions.is_empty() {
            material["extras"]["rim_regions"] = json!(self.rim_regions);
        }
        if self.back_light > 0.0 {
            material["extras"]["rim"] = json!({ "back_light": self.back_light });
        }
        material
    }
}

// Layout in meters, Y-up, facing +Z (the glTF and engine convention), so the figure's
// left is +X. It stands on the pedestal's top face.
const PEDESTAL_HALF_EXTENTS: [f32; 3] = [0.55, 0.1, 0.55];
const PEDESTAL_TOP: f32 = 2.0 * PEDESTAL_HALF_EXTENTS[1];
const HEAD_RADIUS: f32 = 0.15;
/// Head center above the pedestal top.
const HEAD_HEIGHT: f32 = 1.47;
const HAT_HEIGHT: f32 = 0.3;

/// Builds the M0 sample character as a binary glTF (`.glb`).
///
/// A stylized figure that exercises the anime character renderer and the importer:
///
/// - A sphere head (skin, thinner outline via `COLOR_0` green) under a hair cap.
/// - Capsule torso (flattened by a non-uniform node scale) and legs in the outfit color.
/// - Capsule arms in skin, the left one a mirror (negative scale) of the right, in the skin's
///   second region (a `_MATERIAL_ID` of 1), whose rim stays to the outer silhouette.
/// - A hard-edged cone hat in the accent color and a hard-edged box pedestal.
///
/// Shading parameters live in material extras, as a Blender export would carry them.
pub fn mannequin_glb() -> Vec<u8> {
    let mut gltf = GltfWriter::default();
    let [skin, outfit, hair, accent, pedestal] = LOOKS.map(|look| gltf.add_material(look.material()));

    let mut head_geometry = shapes::sphere(HEAD_RADIUS, 32, 24);
    // Unoccluded, the thinner outline, the material's Z offset; glTF requires an alpha.
    head_geometry.colors = vec![[255, HEAD_OUTLINE_WIDTH, 255, 255]; head_geometry.positions.len()];

    let pedestal_mesh =
        gltf.add_mesh("Pedestal", &[(&shapes::cuboid(PEDESTAL_HALF_EXTENTS), Some(pedestal))]);
    let leg_mesh = gltf.add_mesh("Leg", &[(&shapes::capsule(0.075, 0.325, 24, 8), Some(outfit))]);
    let torso_mesh = gltf.add_mesh("Torso", &[(&shapes::capsule(0.17, 0.14, 32, 10), Some(outfit))]);
    let mut arm_geometry = shapes::capsule(0.055, 0.255, 20, 6);
    arm_geometry.material_ids = vec![ARM_REGION; arm_geometry.positions.len()];
    let arm_mesh = gltf.add_mesh("Arm", &[(&arm_geometry, Some(skin))]);
    let head_mesh = gltf.add_mesh("Head", &[(&head_geometry, Some(skin))]);
    let hair_mesh =
        gltf.add_mesh("Hair", &[(&shapes::sphere_cap(0.162, 105f32.to_radians(), 32, 14), Some(hair))]);
    let hat_mesh = gltf.add_mesh("Hat", &[(&shapes::cone(0.12, HAT_HEIGHT, 32), Some(accent))]);

    let mut node = |value: Value| gltf.add_node(value);
    let pedestal = node(json!({
        "name": "Pedestal", "mesh": pedestal_mesh, "translation": [0.0, PEDESTAL_HALF_EXTENTS[1], 0.0],
    }));
    let leg_l = node(json!({ "name": "LegL", "mesh": leg_mesh, "translation": [0.1, 0.4, 0.0] }));
    let leg_r = node(json!({ "name": "LegR", "mesh": leg_mesh, "translation": [-0.1, 0.4, 0.0] }));
    let torso = node(json!({
        "name": "Torso", "mesh": torso_mesh, "translation": [0.0, 1.02, 0.0], "scale": [1.0, 1.0, 0.72],
    }));
    // Arms hang from shoulder pivots in a slight A-pose. The left shoulder mirrors the
    // right one: mirror(T · R(-a)) = T' · R(a) · scale(-1, 1, 1).
    let arm_r = node(json!({ "name": "ArmR", "mesh": arm_mesh, "translation": [0.0, -0.31, 0.0] }));
    let arm_l = node(json!({ "name": "ArmL", "mesh": arm_mesh, "translation": [0.0, -0.31, 0.0] }));
    let shoulder_r = node(json!({
        "name": "ShoulderR", "translation": [-0.2, 1.24, 0.0], "rotation": rotation_z(-15.0),
        "children": [arm_r],
    }));
    let shoulder_l = node(json!({
        "name": "ShoulderL", "translation": [0.2, 1.24, 0.0], "rotation": rotation_z(15.0),
        "scale": [-1.0, 1.0, 1.0], "children": [arm_l],
    }));
    let hair = node(json!({ "name": "Hair", "mesh": hair_mesh, "translation": [0.0, 0.012, -0.008] }));
    let hat = node(json!({
        "name": "Hat", "mesh": hat_mesh, "translation": [0.0, 0.11, 0.0], "rotation": rotation_x(-12.0),
    }));
    let head = node(json!({
        "name": "Head", "mesh": head_mesh, "translation": [0.0, HEAD_HEIGHT, 0.0], "children": [hair, hat],
    }));
    let body = node(json!({
        "name": "Body", "translation": [0.0, PEDESTAL_TOP, 0.0],
        "children": [leg_l, leg_r, torso, shoulder_l, shoulder_r, head],
    }));
    let root = node(json!({ "name": "Mannequin", "children": [pedestal, body] }));
    gltf.add_root(root);
    gltf.to_glb()
}

/// A glTF rotation quaternion (`[x, y, z, w]`) about +Z.
fn rotation_z(degrees: f32) -> [f32; 4] {
    let (sin, cos) = (degrees.to_radians() / 2.0).sin_cos();
    [0.0, 0.0, sin, cos]
}

/// A glTF rotation quaternion (`[x, y, z, w]`) about +X.
fn rotation_x(degrees: f32) -> [f32; 4] {
    let (sin, cos) = (degrees.to_radians() / 2.0).sin_cos();
    [sin, 0.0, 0.0, cos]
}

#[cfg(test)]
mod tests {
    use glam::Vec3;
    use std::collections::BTreeMap;

    use sapphire_assets::material::CharacterRegion;
    use sapphire_assets::mesh::{Bounds, PositionFormat};

    use super::*;
    use crate::import;

    #[test]
    fn is_a_valid_glb() {
        let bytes = mannequin_glb();
        let glb = gltf::Glb::from_slice(&bytes).expect("GLB container");
        assert_eq!(&glb.header.magic, b"glTF");
        assert_eq!(glb.header.version, 2);
        assert_eq!(glb.header.length as usize, bytes.len());
        let gltf = gltf::Gltf::from_slice(&bytes).expect("valid glTF 2.0");
        assert!(gltf.default_scene().is_some());
        assert_eq!(gltf.extensions_required().count(), 0);
    }

    #[test]
    fn reimports_with_materials_parts_and_outline_widths() {
        let mesh = import::gltf::import_slice(&mannequin_glb(), None).expect("re-import");
        let cooked = mesh.pack(PositionFormat::Float32, &BTreeMap::new()).unwrap().to_cooked().expect("cook");
        sapphire_assets::mesh::Mesh::read(&cooked).expect("structurally valid");

        // 9 part instances: pedestal, 2 legs, torso, 2 arms, head, hair, hat.
        assert_eq!(mesh.lods[0].submeshes.len(), 9);
        assert_eq!(mesh.materials.len(), LOOKS.len());
        // Every parameter carries over into the generated anime character materials.
        let rgb = |[r, g, b]: [u8; 3]| {
            let [r, g, b, _] = srgb8(r, g, b);
            [r, g, b]
        };
        for look in LOOKS {
            let slot = mesh.materials.iter().find(|m| m.name == look.name);
            let material = &slot.unwrap_or_else(|| panic!("material {} missing", look.name)).generated;
            let [r, g, b] = look.base;
            assert_eq!(material.base_color, srgb8(r, g, b), "{}", look.name);
            let [threshold, softness, rim, outline] = look.character_shading;
            assert_eq!(
                (material.light_split.threshold, material.light_split.softness),
                (threshold, softness)
            );
            assert_eq!((material.rim.strength, material.outline.width), (rim, outline), "{}", look.name);
            assert_eq!(material.rim.back_light, look.back_light, "{}", look.name);
            let region = CharacterRegion {
                shade_color: rgb(look.shade),
                outline_color: rgb(look.outline),
                ..CharacterRegion::default()
            };
            let regions: Vec<_> = match look.rim_regions {
                [] => vec![region],
                scales => scales
                    .iter()
                    .map(|&[rim_strength, rim_width, rim_overlap]| CharacterRegion {
                        rim_strength,
                        rim_width,
                        rim_overlap,
                        ..region
                    })
                    .collect(),
            };
            assert_eq!(material.regions, regions, "{}", look.name);
        }
        // The arms, and only they, are the skin's second region.
        let arm_vertices = 2 * shapes::capsule(0.055, 0.255, 20, 6).positions.len();
        let in_region = |id: u8| mesh.vertices.iter().filter(|v| v.flags.material_id().get() == id).count();
        assert_eq!(
            (in_region(ARM_REGION as u8), mesh.vertices.len() - in_region(0)),
            (arm_vertices, arm_vertices)
        );

        // Only the head carries a thinner outline, and all of it does.
        let head_center = Vec3::new(0.0, PEDESTAL_TOP + HEAD_HEIGHT, 0.0);
        let thin: Vec<_> = mesh.vertices.iter().filter(|v| v.color[1] == HEAD_OUTLINE_WIDTH).collect();
        assert_eq!(thin.len(), shapes::sphere(HEAD_RADIUS, 32, 24).positions.len());
        for v in &thin {
            assert!((Vec3::from(v.position).distance(head_center) - HEAD_RADIUS).abs() < 1e-4);
        }
        assert!(
            mesh.vertices.iter().all(|v| v.color == [255, HEAD_OUTLINE_WIDTH, 255] || v.color == [255; 3])
        );

        let Bounds { min, max } = Bounds::enclosing(mesh.vertices.iter().map(|v| v.position));
        assert_eq!(min[1], 0.0, "pedestal rests on the ground");
        assert_eq!([min[0], max[0], min[2], max[2]], [-0.55, 0.55, -0.55, 0.55]);
        assert!((2.0..2.2).contains(&max[1]), "hat tip at {}", max[1]);
    }

    #[test]
    fn every_triangle_faces_outward_after_import() {
        // Catches transform, mirroring and winding mistakes anywhere in the hierarchy.
        let mesh = import::gltf::import_slice(&mannequin_glb(), None).unwrap();
        for v in &mesh.vertices {
            assert!((Vec3::from(v.normal).length() - 1.0).abs() < 1e-4);
        }
        for t in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = t.map(|i| &mesh.vertices[i as usize]);
            let face = (Vec3::from(b.position) - Vec3::from(a.position))
                .cross(Vec3::from(c.position) - Vec3::from(a.position));
            let normals = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
            assert!(face.dot(normals) > 0.0, "triangle {t:?} is wound against its normals");
        }
    }

    #[test]
    fn faces_plus_z() {
        // Facing +Z with +Y up puts the figure's left at +X; the hair sits back on the
        // head (toward -Z), leaving the face toward +Z.
        let gltf = gltf::Gltf::from_slice(&mannequin_glb()).unwrap();
        let translation = |name: &str| {
            let node = gltf.nodes().find(|n| n.name() == Some(name)).unwrap_or_else(|| panic!("no {name}"));
            node.transform().decomposed().0
        };
        for side in ["Shoulder", "Leg"] {
            assert!(translation(&format!("{side}L"))[0] > 0.0, "{side}L must be on +X");
            assert!(translation(&format!("{side}R"))[0] < 0.0, "{side}R must be on -X");
        }
        assert!(translation("Hair")[2] < 0.0);
    }

    #[test]
    fn writes_samples_to_disk() {
        let dir = crate::test_util::TempDir::new("samples");
        let nested = dir.path().join("nested");
        let written = write_all(&nested).unwrap();
        let names = [
            MANNEQUIN_FILE_NAME,
            swatch::RAMP_FILE_NAME,
            swatch::ILM_FILE_NAME,
            swatch::MATERIAL_ID_FILE_NAME,
            swatch::SHADOW_BIAS_FILE_NAME,
            swatch::RAMP_MATERIAL_FILE_NAME,
            swatch::SUBSURFACE_MATERIAL_FILE_NAME,
            swatch::BAND_MATERIAL_FILE_NAME,
            sky::CLOUDS_FILE_NAME,
            sky::CIRRUS_FILE_NAME,
            sky::MOON_FILE_NAME,
            sky::STARS_FILE_NAME,
            sky::MILKY_WAY_FILE_NAME,
            sky::MATERIAL_FILE_NAME,
            hard_edges::FILE_NAME,
        ];
        assert_eq!(written[..names.len()], names.map(|name| nested.join(name)));
        let faces: Vec<_> = face::files().unwrap().into_iter().map(|(name, _)| nested.join(name)).collect();
        let environment: Vec<_> =
            environment::files().unwrap().into_iter().map(|(name, _)| nested.join(name)).collect();
        assert_eq!(
            written[names.len()..names.len() + faces.len()],
            faces,
            "then the face's, in their directory"
        );
        let foliage: Vec<_> =
            foliage::files().unwrap().into_iter().map(|(name, _)| nested.join(name)).collect();
        let first = names.len() + faces.len();
        assert_eq!(written[first..first + environment.len()], environment, "then the environment's");
        assert_eq!(written[first + environment.len()..], foliage, "then the foliage's");
        assert!(written.iter().all(|path| path.is_file()));
        assert_eq!(std::fs::read(&written[0]).unwrap(), mannequin_glb(), "generation is deterministic");
    }
}
