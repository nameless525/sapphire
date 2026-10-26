//! The hard-edged outline sample (M1 C4): flat-shaded shapes whose every edge is hard, so every
//! vertex is split once per face that meets at it. The cooker's bake welds them back into one
//! outline normal per position ([`bake`](crate::bake)), stored in each split vertex's own tangent
//! frame, and the inverted hull must stay closed around every corner. The sandbox shows them with
//! `--hard-edges` (the golden scene `outlines-hard-edges`).
//!
//! In a row along X, each turned so its edges meet the camera at different angles: a cube, a
//! hexagonal prism, a triangular prism (60° edges) and an eight-sided gem, each in a material of
//! its own with a dark color line.

use glam::{EulerRot, Quat};
use sapphire_assets::AssetId;
use serde_json::json;

use super::{Look, shapes};
use crate::gltf_writer::GltfWriter;

/// File name of the sample, in the samples directory.
pub const FILE_NAME: &str = "hard_edges.glb";

/// Its ID, which its committed sidecar gives it (`content/samples/hard_edges.glb.meta`).
pub const ID: AssetId = AssetId::from_text("5b0c2d7e-8f41-4a63-9e1d-3c7a2b6f9d40");

/// Height of the shapes' centers above the mesh's origin, and the spacing of the row.
const CENTER_HEIGHT: f32 = 0.45;
const SPACING: f32 = 0.75;

const CUBE: Look = Look {
    name: "Cube",
    base: [232, 120, 96],
    shade: [190, 140, 205],
    outline: [84, 30, 34],
    character_shading: [0.1, 0.02, 0.2, 5.5],
    rim_regions: &[],
    back_light: 0.0,
};
const HEX: Look = Look {
    name: "Hex",
    base: [92, 196, 188],
    shade: [140, 150, 215],
    outline: [18, 58, 70],
    character_shading: [0.1, 0.02, 0.2, 5.5],
    rim_regions: &[],
    back_light: 0.0,
};
const WEDGE: Look = Look {
    name: "Wedge",
    base: [240, 202, 84],
    shade: [215, 160, 150],
    outline: [92, 62, 12],
    character_shading: [0.1, 0.02, 0.2, 5.5],
    rim_regions: &[],
    back_light: 0.0,
};
const GEM: Look = Look {
    name: "Gem",
    base: [172, 122, 232],
    shade: [150, 140, 220],
    outline: [52, 22, 84],
    character_shading: [0.1, 0.02, 0.2, 5.5],
    rim_regions: &[],
    back_light: 0.0,
};

/// Builds the sample as a binary glTF (`.glb`).
pub fn glb() -> Vec<u8> {
    let mut gltf = GltfWriter::default();
    let shapes = [
        ("Cube", &CUBE, shapes::cuboid([0.22; 3]), (35.0, 25.0, 0.0)),
        ("Hex", &HEX, shapes::prism(6, 0.25, 0.26), (0.0, 30.0, 12.0)),
        ("Wedge", &WEDGE, shapes::prism(3, 0.29, 0.24), (20.0, -35.0, 0.0)),
        ("Gem", &GEM, shapes::bipyramid(8, 0.27, 0.34), (10.0, 0.0, 15.0)),
    ];
    let count = shapes.len() as f32;
    let mut children = Vec::new();
    for (index, (name, look, geometry, (yaw, pitch, roll))) in shapes.into_iter().enumerate() {
        let material = gltf.add_material(look.material());
        let mesh = gltf.add_mesh(name, &[(&geometry, Some(material))]);
        let x = (index as f32 - (count - 1.0) / 2.0) * SPACING;
        let rotation = Quat::from_euler(
            EulerRot::YXZ,
            f32::to_radians(yaw),
            f32::to_radians(pitch),
            f32::to_radians(roll),
        );
        children.push(gltf.add_node(json!({
            "name": name,
            "mesh": mesh,
            "translation": [x, CENTER_HEIGHT, 0.0],
            "rotation": rotation.to_array(),
        })));
    }
    let root = gltf.add_node(json!({ "name": "HardEdges", "children": children }));
    gltf.add_root(root);
    gltf.to_glb()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use glam::Vec3;
    use sapphire_assets::mesh::PositionFormat;

    use super::*;
    use crate::bake::{self, OutlineSettings};
    use crate::import;

    #[test]
    fn cooks_one_outline_normal_per_corner_for_every_split_vertex() {
        let mut mesh = import::gltf::import_slice(&glb(), None).expect("imports");
        assert_eq!(mesh.materials.len(), 4);
        bake::outline_normals(&mut mesh, &OutlineSettings::default());
        let cooked = mesh.pack(PositionFormat::Unorm16, &BTreeMap::new()).expect("packs");
        // The split vertices at each position, as the cooked streams decode them.
        let mut corners: HashMap<[u16; 3], Vec<(Vec3, Vec3)>> = HashMap::new();
        let sapphire_assets::mesh::Positions::Quantized(positions) = &cooked.streams.positions else {
            panic!("quantized positions");
        };
        for (index, position) in positions.iter().enumerate() {
            let vertex = cooked.streams.vertex(index).expect("in range");
            corners
                .entry(position.get())
                .or_default()
                .push((Vec3::from(vertex.normal), Vec3::from(vertex.outline_normal)));
        }
        let mut worst = 0.0f32;
        let mut split = 0;
        for vertices in corners.values() {
            let distinct_normals = vertices.iter().any(|(n, _)| n.angle_between(vertices[0].0) > 0.1);
            split += usize::from(distinct_normals);
            for (_, outline) in vertices {
                worst = worst.max(outline.angle_between(vertices[0].1).to_degrees());
            }
        }
        // Every corner is split: each position has several faces' normals.
        assert_eq!(split, corners.len(), "every edge is hard");
        assert!(worst < 0.03, "split vertices decode one outline normal, {worst}° apart at worst");
    }

    #[test]
    fn its_sidecar_gives_it_its_id() {
        let samples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/samples");
        let meta =
            crate::meta::read(&crate::meta::sidecar_path(&samples.join(FILE_NAME))).expect("a sidecar");
        assert_eq!(meta.id, ID);
    }
}
