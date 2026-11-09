//! The warrior's proportions (M2 O4): KayKit's chibi skeleton, re-proportioned at the bind pose into
//! a lanky figure that can stand over the character.
//!
//! The pack's figure has a head as tall as the rest of it, which no material can make threatening.
//! So [`warrior_glb`] derives a model from the pack's, joint by joint (`RESHAPE`): its limbs and
//! ribs longer, along their bones only, its head smaller. Vertices move as the joints they're
//! weighted to do, normals by the inverse transpose, and the rig's rest translations and inverse bind
//! matrices follow, so what comes out is an ordinary skinned model at rest: the same 23 joints with
//! the same names and rest rotations, each at scale 1. It can't share the pack's skeleton, whose rests
//! hold the pack's lengths, and the pack's clips key all 23 joints' translations at those lengths,
//! which would undo all this. So it has a skeleton of its own, and clips reach it by retarget, as they
//! reach any other figure: the pack's clip sidecars name it as a target, by the humanoid map in its
//! own. The accessors keep their layout: only numbers change, so the file is the pack's otherwise.
//!
//! The pack's normals are split at every face, which a two-tone light split turns into a patchwork of
//! facets, the look of a low-poly kit. So bone and cloth smooth across their faces too, up to a
//! crease for each (`crease`), and the terminator runs across them as one painted edge; armour
//! keeps its facets, which read as hammered plate.

use std::collections::BTreeMap;
use std::io;

use glam::{Affine3, Mat3, Mat4, Quat, Vec2, Vec3};
use sapphire_math::det;
use serde_json::{Value, json};

use super::{Part, cell_of, part_of};

/// How the bind pose changes, joint by joint: `(joint, stretch, shift)`. `stretch` scales what's
/// weighted to the joint along its own axes, and its children's places with it, so a limb's bone (its
/// +Y) grows without thickening and without its children growing too. `shift` moves the joint in its
/// parent's space. The legs go first in what the figure needs, from 0.52 to 0.95 at the hip joints;
/// then the head, from 0.93 tall to 0.56.
const RESHAPE: [(&str, [f32; 3], [f32; 3]); 11] = [
    ("spine", [1.0, 1.1, 1.0], [0.0; 3]),
    ("chest", [1.0, 1.25, 1.0], [0.0; 3]),
    ("head", [0.6; 3], [0.0; 3]),
    ("upperarm.l", [1.0, 1.45, 1.0], [0.04, 0.0, 0.0]),
    ("lowerarm.l", [1.0, 1.45, 1.0], [0.0; 3]),
    ("upperarm.r", [1.0, 1.45, 1.0], [-0.04, 0.0, 0.0]),
    ("lowerarm.r", [1.0, 1.45, 1.0], [0.0; 3]),
    ("upperleg.l", [1.0, 2.1, 1.0], [0.0; 3]),
    ("lowerleg.l", [1.0, 2.1, 1.0], [0.0; 3]),
    ("upperleg.r", [1.0, 2.1, 1.0], [0.0; 3]),
    ("lowerleg.r", [1.0, 2.1, 1.0], [0.0; 3]),
];

/// The angle between two faces of a part, in degrees, up to which they shade as one surface: its
/// cosine, or `None` for a part that keeps its facets.
fn crease(part: Part) -> Option<f32> {
    let degrees: f32 = match part {
        Part::Cloak => 80.0,
        Part::Bone => 50.0,
        Part::Armour | Part::Steel | Part::Leather | Part::Trim => return None,
    };
    Some(det::cos(degrees.to_radians()))
}

/// Smooths one primitive's normals, in place, across the vertices of a part that lie in one place.
fn smooth(uvs: &[Vec2], positions: &[Vec3], normals: &mut [Vec3]) {
    // In order of their places, so the sum never depends on a hash.
    let mut places: BTreeMap<(i32, i32, i32, u32, u32), Vec<usize>> = BTreeMap::new();
    let mut parts = Vec::with_capacity(uvs.len());
    for (vertex, (&uv, &position)) in uvs.iter().zip(positions).enumerate() {
        let (column, row, _) = cell_of(uv);
        parts.push(part_of(column, row));
        let quantized = (position * 2048.0).round().as_ivec3();
        places.entry((quantized.x, quantized.y, quantized.z, column, row)).or_default().push(vertex);
    }
    let before = normals.to_vec();
    for vertices in places.values() {
        for &vertex in vertices {
            let Some(limit) = parts[vertex].and_then(crease) else { continue };
            let sum: Vec3 = vertices
                .iter()
                .map(|&other| before[other])
                .filter(|other| other.dot(before[vertex]) >= limit)
                .sum();
            normals[vertex] = sum.normalize_or(before[vertex]);
        }
    }
}

/// The joint the whole figure rises by, to keep its soles where they were.
const LIFTED: &str = "hips";

fn bad(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("the warrior's source: {}", what.into()))
}

/// A GLB's JSON and binary chunks.
fn split(glb: &[u8]) -> io::Result<(Value, Vec<u8>)> {
    let glb = ::gltf::Glb::from_slice(glb).map_err(|error| bad(error.to_string()))?;
    let json = serde_json::from_slice(&glb.json).map_err(|error| bad(error.to_string()))?;
    let bin = glb.bin.ok_or_else(|| bad("no binary chunk"))?.into_owned();
    Ok((json, bin))
}

/// A GLB from its chunks: the JSON padded with spaces, the binary with zeros, as the format says.
fn join(json: &Value, bin: &[u8]) -> io::Result<Vec<u8>> {
    let mut text = serde_json::to_vec(json).map_err(io::Error::other)?;
    text.resize(text.len().next_multiple_of(4), b' ');
    let bin_length = bin.len().next_multiple_of(4);
    let total = 12 + 8 + text.len() + 8 + bin_length;
    let size = |length: usize| u32::try_from(length).map_err(|_| bad("too large")).map(u32::to_le_bytes);
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&size(total)?);
    out.extend_from_slice(&size(text.len())?);
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&size(bin_length)?);
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(bin);
    out.resize(total, 0);
    Ok(out)
}

/// Where an accessor's elements lie in the binary chunk.
struct Elements {
    start: usize,
    stride: usize,
    count: usize,
    component: u64,
}

impl Elements {
    fn of(json: &Value, accessor: u64) -> io::Result<Self> {
        let accessor = &json["accessors"][accessor as usize];
        let view = &json["bufferViews"]
            [accessor["bufferView"].as_u64().ok_or_else(|| bad("a sparse accessor"))? as usize];
        let component = accessor["componentType"].as_u64().ok_or_else(|| bad("an accessor with no type"))?;
        let size = match component {
            5121 => 1,
            5123 => 2,
            5126 => 4,
            other => return Err(bad(format!("component type {other}"))),
        };
        let parts = match accessor["type"].as_str() {
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            other => return Err(bad(format!("accessor type {other:?}"))),
        };
        let offset = |value: &Value| value.as_u64().unwrap_or(0) as usize;
        Ok(Self {
            start: offset(&view["byteOffset"]) + offset(&accessor["byteOffset"]),
            stride: view["byteStride"].as_u64().map_or(size * parts, |stride| stride as usize),
            count: accessor["count"].as_u64().ok_or_else(|| bad("an accessor with no count"))? as usize,
            component,
        })
    }

    fn floats<const N: usize>(&self, bin: &[u8], element: usize) -> [f32; N] {
        let at = self.start + element * self.stride;
        std::array::from_fn(|part| {
            let start = at + part * 4;
            f32::from_le_bytes([bin[start], bin[start + 1], bin[start + 2], bin[start + 3]])
        })
    }

    fn write_floats(&self, bin: &mut [u8], element: usize, values: &[f32]) {
        let at = self.start + element * self.stride;
        for (part, value) in values.iter().enumerate() {
            bin[at + part * 4..at + part * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
    }

    /// An element's four unsigned parts, as `JOINTS_0` holds them.
    fn indices(&self, bin: &[u8], element: usize) -> io::Result<[usize; 4]> {
        let at = self.start + element * self.stride;
        match self.component {
            5121 => Ok(std::array::from_fn(|part| usize::from(bin[at + part]))),
            5123 => Ok(std::array::from_fn(|part| {
                usize::from(u16::from_le_bytes([bin[at + part * 2], bin[at + part * 2 + 1]]))
            })),
            other => Err(bad(format!("joint indices of component type {other}"))),
        }
    }
}

/// A node of the scene, with what [`RESHAPE`] says of it.
#[derive(Clone, Debug)]
struct Node {
    translation: Vec3,
    rotation: Quat,
    scale: Vec3,
    parent: Option<usize>,
    stretch: Vec3,
    shift: Vec3,
}

fn vector(value: &Value, default: Vec3) -> Vec3 {
    let part = |index: usize, default: f32| value[index].as_f64().map_or(default, |number| number as f32);
    Vec3::new(part(0, default.x), part(1, default.y), part(2, default.z))
}

fn nodes_of(json: &Value) -> io::Result<Vec<Node>> {
    let list = json["nodes"].as_array().ok_or_else(|| bad("no nodes"))?;
    let mut nodes: Vec<Node> = list
        .iter()
        .map(|node| {
            let rotation = &node["rotation"];
            let part =
                |index: usize, default: f32| rotation[index].as_f64().map_or(default, |number| number as f32);
            Node {
                translation: vector(&node["translation"], Vec3::ZERO),
                rotation: det::quat_normalize(Quat::from_xyzw(
                    part(0, 0.0),
                    part(1, 0.0),
                    part(2, 0.0),
                    part(3, 1.0),
                )),
                scale: vector(&node["scale"], Vec3::ONE),
                parent: None,
                stretch: Vec3::ONE,
                shift: Vec3::ZERO,
            }
        })
        .collect();
    for (parent, node) in list.iter().enumerate() {
        for child in node["children"].as_array().into_iter().flatten() {
            let child = child.as_u64().ok_or_else(|| bad("a child that isn't an index"))? as usize;
            nodes.get_mut(child).ok_or_else(|| bad("a child that isn't a node"))?.parent = Some(parent);
        }
    }
    Ok(nodes)
}

/// A node's place in the model with [`RESHAPE`]'s changes, in scalar arithmetic: `Mat4`'s SIMD
/// backends round differently, and the committed model is pinned by bytes on every OS.
fn world(nodes: &[Node], node: usize) -> Affine3 {
    let own = &nodes[node];
    let (parent, stretch) = match own.parent {
        Some(parent) => (world(nodes, parent), nodes[parent].stretch),
        None => (Affine3::IDENTITY, Vec3::ONE),
    };
    let translation = own.translation * stretch + own.shift;
    parent * Affine3::from_scale_rotation_translation(own.scale, own.rotation, translation)
}

/// The lowest any skinned vertex lies, by the accessors' bounds.
fn lowest(json: &Value) -> io::Result<f32> {
    let nodes = json["nodes"].as_array().ok_or_else(|| bad("no nodes"))?;
    let meshes = nodes.iter().filter(|node| node["skin"].is_u64()).filter_map(|node| node["mesh"].as_u64());
    let mut lowest = f32::MAX;
    for mesh in meshes {
        for primitive in json["meshes"][mesh as usize]["primitives"].as_array().into_iter().flatten() {
            let position = primitive["attributes"]["POSITION"].as_u64().ok_or_else(|| bad("no POSITION"))?;
            let bound = json["accessors"][position as usize]["min"][1].as_f64();
            lowest = lowest.min(bound.ok_or_else(|| bad("a POSITION with no bounds"))? as f32);
        }
    }
    Ok(lowest)
}

/// The warrior as the enemy wears it, from the pack's GLB: see the [module docs](self). It stands on
/// the pack's ground: everything rests under `LIFTED`, so raising that raises all of it alike.
pub fn warrior_glb(source: &[u8]) -> io::Result<Vec<u8>> {
    let ground = lowest(&split(source)?.0)?;
    let raise = ground - lowest(&reshaped(source, 0.0)?.0)?;
    let (json, bin) = reshaped(source, raise)?;
    join(&json, &bin)
}

/// The pack's GLB through [`RESHAPE`], with [`LIFTED`] `raise` higher.
fn reshaped(source: &[u8], raise: f32) -> io::Result<(Value, Vec<u8>)> {
    let (mut json, mut bin) = split(source)?;
    let mut nodes = nodes_of(&json)?;
    let names: Vec<String> = json["nodes"]
        .as_array()
        .ok_or_else(|| bad("no nodes"))?
        .iter()
        .map(|node| node["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    let find = |name: &str| {
        names.iter().position(|known| known == name).ok_or_else(|| bad(format!("no joint named {name:?}")))
    };
    for (joint, stretch, shift) in RESHAPE {
        let node = &mut nodes[find(joint)?];
        (node.stretch, node.shift) = (Vec3::from(stretch), Vec3::from(shift));
    }
    nodes[find(LIFTED)?].shift.y += raise;

    let skin = json["skins"][0].clone();
    let joints: Vec<usize> = skin["joints"]
        .as_array()
        .ok_or_else(|| bad("no skin"))?
        .iter()
        .map(|joint| {
            joint.as_u64().map(|joint| joint as usize).ok_or_else(|| bad("a joint that isn't an index"))
        })
        .collect::<io::Result<_>>()?;
    let inverse_binds =
        Elements::of(&json, skin["inverseBindMatrices"].as_u64().ok_or_else(|| bad("no binds"))?)?;
    let new_world: Vec<Affine3> = joints.iter().map(|&joint| world(&nodes, joint)).collect();
    // What moves each joint's vertices from the bind pose to the new one.
    let moves: Vec<Affine3> = joints
        .iter()
        .enumerate()
        .map(|(index, &joint)| {
            let bind = Affine3::from_mat4(Mat4::from_cols_array(&inverse_binds.floats::<16>(&bin, index)));
            new_world[index] * Affine3::from_scale(nodes[joint].stretch) * bind
        })
        .collect();
    let normals: Vec<Mat3> = moves.iter().map(|moved| moved.matrix3.inverse().transpose()).collect();

    // Every skinned primitive's vertices, once each.
    let meshes: Vec<u64> = json["nodes"]
        .as_array()
        .ok_or_else(|| bad("no nodes"))?
        .iter()
        .filter(|node| node["skin"].is_u64())
        .filter_map(|node| node["mesh"].as_u64())
        .collect();
    for mesh in meshes {
        let primitives = json["meshes"][mesh as usize]["primitives"].as_array().cloned().unwrap_or_default();
        for primitive in primitives {
            let attribute = |name: &str| {
                primitive["attributes"][name]
                    .as_u64()
                    .ok_or_else(|| bad(format!("a primitive with no {name}")))
            };
            let position = attribute("POSITION")?;
            let (positions, normal_elements) =
                (Elements::of(&json, position)?, Elements::of(&json, attribute("NORMAL")?)?);
            let (joint_elements, weights) =
                (Elements::of(&json, attribute("JOINTS_0")?)?, Elements::of(&json, attribute("WEIGHTS_0")?)?);
            let uv_elements = Elements::of(&json, attribute("TEXCOORD_0")?)?;
            let uvs: Vec<Vec2> = (0..positions.count)
                .map(|vertex| Vec2::from(uv_elements.floats::<2>(&bin, vertex)))
                .collect();
            let points: Vec<Vec3> =
                (0..positions.count).map(|vertex| Vec3::from(positions.floats::<3>(&bin, vertex))).collect();
            let mut smoothed: Vec<Vec3> = (0..positions.count)
                .map(|vertex| Vec3::from(normal_elements.floats::<3>(&bin, vertex)))
                .collect();
            smooth(&uvs, &points, &mut smoothed);
            let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for vertex in 0..positions.count {
                let (point, normal) = (points[vertex], smoothed[vertex]);
                let influences = joint_elements.indices(&bin, vertex)?;
                let weight = weights.floats::<4>(&bin, vertex);
                let (mut new_point, mut new_normal) = (Vec3::ZERO, Vec3::ZERO);
                for (&joint, &weight) in influences.iter().zip(&weight) {
                    if weight > 0.0 {
                        let moved = moves.get(joint).ok_or_else(|| bad("a weight on no joint"))?;
                        new_point += moved.transform_point3(point) * weight;
                        new_normal += normals[joint] * normal * weight;
                    }
                }
                let new_normal = new_normal.normalize_or(normal);
                positions.write_floats(&mut bin, vertex, &new_point.to_array());
                normal_elements.write_floats(&mut bin, vertex, &new_normal.to_array());
                (min, max) = (min.min(new_point), max.max(new_point));
            }
            let floats = |value: Vec3| value.to_array().map(f64::from);
            json["accessors"][position as usize]["min"] = json!(floats(min));
            json["accessors"][position as usize]["max"] = json!(floats(max));
        }
    }

    // The rig at rest in the new pose.
    for (index, world) in new_world.iter().enumerate() {
        inverse_binds.write_floats(&mut bin, index, &Mat4::from(world.inverse()).to_cols_array());
    }
    for (index, node) in nodes.iter().enumerate() {
        let stretch = node.parent.map_or(Vec3::ONE, |parent| nodes[parent].stretch);
        let translation = node.translation * stretch + node.shift;
        if translation != node.translation {
            json["nodes"][index]["translation"] = json!(translation.to_array().map(f64::from));
        }
        // A mesh rigidly under a stretched joint, such as the helmet, takes the stretch itself.
        if stretch != Vec3::ONE && json["nodes"][index]["mesh"].is_u64() {
            json["nodes"][index]["scale"] = json!((node.scale * stretch).to_array().map(f64::from));
        }
    }
    Ok((json, bin))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::gltf;

    fn source() -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/kaykit_skeletons/characters/Skeleton_Warrior.glb");
        std::fs::read(path).expect("the pack's warrior")
    }

    /// The figure the derived model makes, in the model's units.
    fn height(mesh: &crate::mesh::ImportedMesh) -> f32 {
        mesh.vertices.iter().map(|vertex| vertex.position[1]).fold(0.0, f32::max)
    }

    #[test]
    fn the_reshaped_warrior_is_an_ordinary_skinned_model_at_rest() {
        let bytes = source();
        let before = gltf::import_slice(&bytes, None).expect("the pack's imports");
        let after = gltf::import_slice(&warrior_glb(&bytes).unwrap(), None).expect("the derived imports");
        let (old, new) = (before.skin.as_ref().unwrap(), after.skin.as_ref().unwrap());
        assert_eq!(new.joints.len(), old.joints.len(), "the same joints");
        assert_eq!(after.vertices.len(), before.vertices.len(), "the same vertices");
        assert_eq!(after.indices, before.indices, "the same triangles");
        for (old, new) in old.joints.iter().zip(&new.joints) {
            assert_eq!((&old.name, old.parent), (&new.name, new.parent));
            assert_eq!(
                old.rest.rotation, new.rest.rotation,
                "{}: clips' rotations play as they did",
                old.name
            );
            assert_eq!(old.rest.scale, new.rest.scale, "{}: at scale 1", old.name);
        }
        // At rest every bind is the identity, so what the file holds is what draws.
        let worlds = rest_worlds(new);
        for bind in &new.binds {
            let rest = worlds[usize::from(bind.joint)] * Mat4::from_cols_array_2d(&bind.inverse_bind_matrix);
            assert!(rest.abs_diff_eq(Mat4::IDENTITY, 2e-4), "{:?}: {rest}", bind.joint);
        }
    }

    fn rest_worlds(skin: &crate::mesh::ImportedSkin) -> Vec<Mat4> {
        let mut worlds: Vec<Mat4> = Vec::new();
        for joint in &skin.joints {
            let own = Mat4::from_scale_rotation_translation(
                Vec3::from(joint.rest.scale),
                Quat::from_array(joint.rest.rotation),
                Vec3::from(joint.rest.translation),
            );
            worlds.push(joint.parent.map_or(own, |parent| worlds[usize::from(parent)] * own));
        }
        worlds
    }

    /// Bone and cloth shade across their faces and armour keeps its facets, up to each part's crease.
    #[test]
    fn smoothing_follows_each_part_s_crease() {
        // Two faces meeting at 40°, once in bone's cell, once in armour's.
        let (bone, armour) = (Vec2::new(1.5 / 8.0, 1.5 / 4.0), Vec2::new(3.5 / 8.0, 0.5 / 4.0));
        let turned = Quat::from_rotation_x(40f32.to_radians()) * Vec3::Y;
        let positions = [Vec3::X; 4];
        let mut normals = [Vec3::Y, turned, Vec3::Y, turned];
        smooth(&[bone, bone, armour, armour], &positions, &mut normals);
        let middle = (Vec3::Y + turned).normalize();
        assert!(normals[0].abs_diff_eq(middle, 1e-5) && normals[1].abs_diff_eq(middle, 1e-5));
        assert_eq!((normals[2], normals[3]), (Vec3::Y, turned), "armour keeps its facets");
        // Past bone's crease, a corner stays a corner.
        let corner = Quat::from_rotation_x(70f32.to_radians()) * Vec3::Y;
        let mut normals = [Vec3::Y, corner];
        smooth(&[bone, bone], &positions[..2], &mut normals);
        assert_eq!(normals, [Vec3::Y, corner]);
    }

    #[test]
    fn it_stands_on_the_same_ground_longer_in_the_leg_and_smaller_in_the_head() {
        let bytes = source();
        let before = gltf::import_slice(&bytes, None).expect("the pack's imports");
        let after = gltf::import_slice(&warrior_glb(&bytes).unwrap(), None).expect("the derived imports");
        let lowest = |mesh: &crate::mesh::ImportedMesh| {
            mesh.vertices.iter().map(|vertex| vertex.position[1]).fold(f32::MAX, f32::min)
        };
        assert!((lowest(&after) - lowest(&before)).abs() < 1e-3, "its feet stay on the ground");
        let joint_y = |mesh: &crate::mesh::ImportedMesh, name: &str| {
            let skin = mesh.skin.as_ref().unwrap();
            let index = skin.joints.iter().position(|joint| joint.name == name).expect(name);
            rest_worlds(skin)[index].w_axis.y
        };
        assert!(joint_y(&after, "hips") > 1.9 * joint_y(&before, "hips"), "hips well up");
        // Head to helmet's top: a good deal less of the figure than the pack's half.
        let head = |mesh: &crate::mesh::ImportedMesh| (height(mesh) - joint_y(mesh, "head")) / height(mesh);
        assert!(head(&before) > 0.5 && head(&after) < 0.33, "{} to {}", head(&before), head(&after));
    }
}
