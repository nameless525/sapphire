//! Test VRM files, built in code: no third-party VRM is committed or downloaded.
//!
//! [`vrm1`] and [`vrm0`] describe the same small character, as VRM 1.0 and as UniVRM 0.x
//! exported it: the 0.x scene faces −Z (every position, normal and node translation turned half
//! a turn about Y) and keeps its data in the `VRM` extension, so a migrated 0.x import should
//! match the 1.0 one. Tests edit [`SampleVrm::document`] to build broken files.
//!
//! The character: a `Root` node over a humanoid skeleton (hips to hands, chest, neck, two left
//! thumb bones), a forearm twist bone, a hair chain `Hair1 → Hair2 → {Hair3 → Hair3_end,
//! HairSide}` whose end node no skin uses, and three meshes: a skinned `Body` (materials Skin,
//! Hair and Plain), a static `Face` (Skin, morph targets `blink` and `smile`) and static `Lashes`
//! (Lashes, a `blink` of its own). Materials: Skin is a textured MToon, Hair an untextured one,
//! Lashes unlit with a texture, Plain plain glTF, and Unused a textured MToon no primitive uses.

use std::io::Cursor;

use glam::Vec3;
use serde_json::{Value, json};

use crate::gltf_writer::{Geometry, GltfWriter, InfluenceSet, MorphGeometry, glb};

/// Node indices.
pub(crate) mod node {
    pub const ROOT: usize = 0;
    pub const HIPS: usize = 1;
    pub const SPINE: usize = 2;
    pub const CHEST: usize = 3;
    pub const NECK: usize = 4;
    pub const HEAD: usize = 5;
    pub const LEFT_HAND: usize = 14;
    pub const TWIST: usize = 18;
    pub const HAIR1: usize = 19;
    pub const HAIR2: usize = 20;
    pub const HAIR3: usize = 21;
    pub const HAIR3_END: usize = 22;
    pub const BODY: usize = 23;
    pub const FACE: usize = 24;
    pub const LASHES: usize = 25;
    pub const LEFT_THUMB_A: usize = 26;
    pub const LEFT_THUMB_B: usize = 27;
    pub const HAIR_SIDE: usize = 28;
}

/// Material indices.
pub(crate) mod material {
    pub const SKIN: usize = 0;
    pub const HAIR: usize = 1;
    pub const LASHES: usize = 2;
    pub const PLAIN: usize = 3;
    pub const UNUSED: usize = 4;
}

/// Image (and texture) indices.
pub(crate) mod image {
    pub const ALBEDO: usize = 0;
    pub const SHIFT: usize = 1;
    pub const NORMAL: usize = 2;
    pub const RIM: usize = 3;
    /// Outline width in G, rising from 0 at the left edge to 1 at the right.
    pub const OUTLINE: usize = 4;
    /// A JPEG.
    pub const MATCAP: usize = 5;
    /// Outline width in R, as VRM 0.x reads it.
    pub const OUTLINE_R: usize = 6;
    /// Every image's size.
    pub const SIZE: u32 = 8;
}

/// The skeleton: name, parent and translation (as VRM 1.0 lays it out, facing +Z).
const NODES: &[(&str, Option<usize>, [f32; 3])] = &[
    ("Root", None, [0.0, 0.0, 0.0]),
    ("Hips", Some(node::ROOT), [0.0, 0.9, 0.0]),
    ("Spine", Some(node::HIPS), [0.0, 0.1, 0.0]),
    ("Chest", Some(node::SPINE), [0.0, 0.15, 0.0]),
    ("Neck", Some(node::CHEST), [0.0, 0.2, 0.0]),
    ("Head", Some(node::NECK), [0.0, 0.1, 0.0]),
    ("LeftUpperLeg", Some(node::HIPS), [0.1, -0.05, 0.0]),
    ("LeftLowerLeg", Some(6), [0.0, -0.4, 0.0]),
    ("LeftFoot", Some(7), [0.0, -0.4, 0.05]),
    ("RightUpperLeg", Some(node::HIPS), [-0.1, -0.05, 0.0]),
    ("RightLowerLeg", Some(9), [0.0, -0.4, 0.0]),
    ("RightFoot", Some(10), [0.0, -0.4, 0.05]),
    ("LeftUpperArm", Some(node::CHEST), [0.2, 0.15, 0.0]),
    ("LeftLowerArm", Some(12), [0.25, 0.0, 0.0]),
    ("LeftHand", Some(13), [0.25, 0.0, 0.0]),
    ("RightUpperArm", Some(node::CHEST), [-0.2, 0.15, 0.0]),
    ("RightLowerArm", Some(15), [-0.25, 0.0, 0.0]),
    ("RightHand", Some(16), [-0.25, 0.0, 0.0]),
    ("LeftLowerArmTwist", Some(13), [0.12, 0.0, 0.0]),
    ("Hair1", Some(node::HEAD), [0.0, 0.1, -0.05]),
    ("Hair2", Some(node::HAIR1), [0.0, -0.1, -0.02]),
    ("Hair3", Some(node::HAIR2), [0.0, -0.1, 0.0]),
    ("Hair3_end", Some(node::HAIR3), [0.0, -0.05, 0.0]),
    ("Body", Some(node::ROOT), [0.0; 3]),
    ("Face", Some(node::ROOT), [0.0; 3]),
    ("Lashes", Some(node::ROOT), [0.0; 3]),
    ("LeftThumbA", Some(node::LEFT_HAND), [0.03, 0.0, 0.03]),
    ("LeftThumbB", Some(node::LEFT_THUMB_A), [0.03, 0.0, 0.0]),
    ("HairSide", Some(node::HAIR2), [0.05, -0.05, 0.0]),
];

/// The nodes `Body`'s skin uses: every humanoid and hair node but `Hair3_end` and the meshes.
fn skin_joints() -> Vec<usize> {
    (node::HIPS..=node::HAIR3).chain([node::LEFT_THUMB_A, node::LEFT_THUMB_B, node::HAIR_SIDE]).collect()
}

/// The humanoid's bones by VRM 1.0 name, with their nodes.
const HUMANOID: &[(&str, usize)] = &[
    ("hips", node::HIPS),
    ("spine", node::SPINE),
    ("chest", node::CHEST),
    ("neck", node::NECK),
    ("head", node::HEAD),
    ("leftUpperLeg", 6),
    ("leftLowerLeg", 7),
    ("leftFoot", 8),
    ("rightUpperLeg", 9),
    ("rightLowerLeg", 10),
    ("rightFoot", 11),
    ("leftUpperArm", 12),
    ("leftLowerArm", 13),
    ("leftHand", node::LEFT_HAND),
    ("rightUpperArm", 15),
    ("rightLowerArm", 16),
    ("rightHand", 17),
];

/// A sample VRM file before packing.
pub(crate) struct SampleVrm {
    /// The glTF JSON.
    pub document: Value,
    pub bin: Vec<u8>,
}

impl SampleVrm {
    pub fn to_glb(&self) -> Vec<u8> {
        glb(&self.document, &self.bin)
    }

    /// Adds a PNG image named `name` in a buffer view of its own, and a texture showing it;
    /// returns the texture's index (also the image's, as for the sample's own).
    pub fn add_png_texture(&mut self, name: &str, png: &[u8]) -> usize {
        let view = self.document["bufferViews"].as_array().expect("buffer views").len();
        let offset = self.bin.len();
        self.bin.extend_from_slice(png);
        self.document["buffers"][0]["byteLength"] = self.bin.len().into();
        let push = |array: &mut Value, value: Value| {
            let array = array.as_array_mut().expect("an array");
            array.push(value);
            array.len() - 1
        };
        push(
            &mut self.document["bufferViews"],
            json!({ "buffer": 0, "byteOffset": offset, "byteLength": png.len() }),
        );
        let image = push(
            &mut self.document["images"],
            json!({ "name": name, "mimeType": "image/png", "bufferView": view }),
        );
        let texture = push(&mut self.document["textures"], json!({ "source": image }));
        assert_eq!(texture, image, "the sample's textures and images pair by index");
        texture
    }
}

/// The VRM 1.0 character.
pub(crate) fn vrm1() -> SampleVrm {
    let mut writer = scene(false);
    for (index, value) in materials_v1().into_iter().enumerate() {
        assert_eq!(writer.add_material(value), index);
    }
    writer.use_extension("VRMC_materials_mtoon");
    writer.use_extension("KHR_materials_unlit");
    writer.use_extension("VRMC_node_constraint");
    let humanoid: serde_json::Map<String, Value> =
        HUMANOID.iter().map(|&(name, node)| (name.to_owned(), json!({ "node": node }))).collect();
    writer.set_extension(
        "VRMC_vrm",
        json!({
            "specVersion": "1.0",
            "meta": { "name": "Sample", "authors": ["Sapphire"], "licenseUrl": "https://vrm.dev/licenses/1.0/" },
            "humanoid": { "humanBones": humanoid },
            "expressions": {
                "preset": {
                    "happy": { "morphTargetBinds": [{ "node": node::FACE, "index": 1, "weight": 1.0 }] },
                    "blink": {
                        "morphTargetBinds": [
                            { "node": node::FACE, "index": 0, "weight": 1.0 },
                            { "node": node::LASHES, "index": 0, "weight": 0.8 }
                        ],
                        "overrideMouth": "block"
                    },
                    "aa": {
                        "isBinary": true,
                        "morphTargetBinds": [{ "node": node::FACE, "index": 1, "weight": 0.5 }]
                    }
                },
                "custom": {
                    "sparkle": {
                        "overrideBlink": "blend",
                        "materialColorBinds": [
                            { "material": material::SKIN, "type": "shadeColor", "targetValue": [1.0, 0.5, 0.5, 1.0] },
                            { "material": material::UNUSED, "type": "color", "targetValue": [1.0, 1.0, 1.0, 1.0] }
                        ],
                        "textureTransformBinds": [
                            { "material": material::HAIR, "scale": [1.0, 1.0], "offset": [0.5, 0.0] }
                        ]
                    }
                }
            },
            "lookAt": {
                "offsetFromHeadBone": [0.0, 0.06, 0.03],
                "type": "bone",
                "rangeMapHorizontalInner": { "inputMaxValue": 90.0, "outputScale": 8.0 },
                "rangeMapHorizontalOuter": { "inputMaxValue": 90.0, "outputScale": 12.0 },
                "rangeMapVerticalDown": { "inputMaxValue": 60.0, "outputScale": 10.0 },
                "rangeMapVerticalUp": { "inputMaxValue": 60.0, "outputScale": 10.0 }
            }
        }),
    );
    writer.set_extension(
        "VRMC_springBone",
        json!({
            "specVersion": "1.0",
            "colliders": [
                { "node": node::HEAD, "shape": { "sphere": { "offset": [0.0, 0.05, 0.0], "radius": 0.1 } } },
                {
                    "node": node::CHEST,
                    "shape": { "capsule": { "offset": [0.0, 0.0, 0.0], "radius": 0.12, "tail": [0.0, 0.15, 0.0] } }
                }
            ],
            "colliderGroups": [{ "name": "upper body", "colliders": [0, 1] }],
            "springs": [{
                "name": "hair",
                "joints": [
                    { "node": node::HAIR1, "hitRadius": 0.02, "stiffness": 1.5, "gravityPower": 0.2,
                      "gravityDir": [0.0, -1.0, 0.0], "dragForce": 0.4 },
                    { "node": node::HAIR2 },
                    { "node": node::HAIR3 },
                    { "node": node::HAIR3_END }
                ],
                "colliderGroups": [0],
                "center": node::HIPS
            }]
        }),
    );
    finish(writer)
}

/// The same character as UniVRM 0.x exported it.
pub(crate) fn vrm0() -> SampleVrm {
    let mut writer = scene(true);
    for (index, value) in materials_v0().into_iter().enumerate() {
        assert_eq!(writer.add_material(value), index);
    }
    writer.use_extension("KHR_materials_unlit");
    let mut bones: Vec<Value> = HUMANOID
        .iter()
        .map(|&(bone, node)| json!({ "bone": bone, "node": node, "useDefaultValues": true }))
        .collect();
    // 0.x's thumb names: proximal and intermediate, which 1.0 calls metacarpal and proximal.
    bones.push(json!({ "bone": "leftThumbProximal", "node": node::LEFT_THUMB_A }));
    bones.push(json!({ "bone": "leftThumbIntermediate", "node": node::LEFT_THUMB_B }));
    let face_mesh = 1;
    let mtoon = |name: &str, floats: Value, vectors: Value, textures: Value| {
        json!({
            "name": name, "shader": "VRM/MToon", "renderQueue": 2000,
            "floatProperties": floats, "vectorProperties": vectors, "textureProperties": textures,
            "keywordMap": {}, "tagMap": { "RenderType": "Opaque" }
        })
    };
    writer.set_extension(
        "VRM",
        json!({
            "exporterVersion": "UniVRM-0.99.0",
            "specVersion": "0.0",
            "meta": { "title": "Sample", "author": "Sapphire", "licenseName": "CC0" },
            "humanoid": { "humanBones": bones },
            "firstPerson": {
                "firstPersonBone": node::HEAD,
                "firstPersonBoneOffset": { "x": 0.01, "y": 0.06, "z": 0.03 },
                "lookAtTypeName": "Bone",
                "lookAtHorizontalInner": { "curve": [0, 0, 0, 1, 1, 1, 1, 0], "xRange": 90, "yRange": 8 },
                "lookAtHorizontalOuter": { "xRange": 90, "yRange": 12 },
                "lookAtVerticalDown": { "xRange": 60, "yRange": 10 },
                "lookAtVerticalUp": { "xRange": 60, "yRange": 10 }
            },
            "blendShapeMaster": { "blendShapeGroups": [
                { "name": "Joy", "presetName": "joy", "binds": [{ "mesh": face_mesh, "index": 1, "weight": 100 }] },
                { "name": "Blink_L", "presetName": "blink_l", "binds": [{ "mesh": face_mesh, "index": 0, "weight": 50 }] },
                {
                    "name": "Sparkle", "presetName": "unknown", "isBinary": true, "binds": [],
                    "materialValues": [
                        { "materialName": "Skin", "propertyName": "_ShadeColor", "targetValue": [1.0, 0.5, 0.5, 1.0] },
                        { "materialName": "Hair", "propertyName": "_MainTex_ST", "targetValue": [1.0, 0.5, 0.25, 0.1] },
                        { "materialName": "Hair", "propertyName": "_SomethingElse", "targetValue": [1.0, 1.0, 1.0, 1.0] }
                    ]
                }
            ]},
            "secondaryAnimation": {
                "boneGroups": [{
                    "comment": "hair", "stiffiness": 1.5, "gravityPower": 0.2,
                    "gravityDir": { "x": 0.1, "y": -1.0, "z": 0.2 }, "dragForce": 0.4, "center": -1,
                    "hitRadius": 0.02, "bones": [node::HAIR1], "colliderGroups": [0]
                }],
                "colliderGroups": [{
                    "node": node::HEAD,
                    "colliders": [{ "offset": { "x": 0.01, "y": 0.05, "z": 0.02 }, "radius": 0.1 }]
                }]
            },
            "materialProperties": [
                mtoon(
                    "Skin",
                    json!({
                        "_ShadeToony": 0.8, "_ShadeShift": -0.1, "_ShadingGradeRate": 0.5, "_BlendMode": 0,
                        "_RimFresnelPower": 2.0, "_RimLift": 0.1, "_RimLightingMix": 0.5,
                        "_OutlineWidthMode": 1, "_OutlineWidth": 0.2, "_OutlineColorMode": 1,
                        "_OutlineLightingMix": 1.0
                    }),
                    json!({
                        "_Color": [0.9, 0.8, 1.0, 1.0], "_ShadeColor": [0.6, 0.5, 0.7, 1.0],
                        "_RimColor": [0.5, 0.25, 0.25, 1.0], "_OutlineColor": [0.2, 0.1, 0.1, 1.0],
                        "_MainTex": [0.0, 0.0, 1.0, 1.0]
                    }),
                    json!({
                        "_MainTex": image::ALBEDO, "_ShadeTexture": image::ALBEDO, "_BumpMap": image::NORMAL,
                        "_ShadingGradeTexture": image::SHIFT, "_RimTexture": image::RIM,
                        "_SphereAdd": image::MATCAP, "_OutlineWidthTexture": image::OUTLINE_R
                    }),
                ),
                mtoon(
                    "Hair",
                    json!({ "_OutlineWidthMode": 2, "_OutlineWidth": 0.8, "_BlendMode": 1, "_Cutoff": 0.3 }),
                    json!({ "_Color": [0.5, 0.5, 0.5, 1.0], "_ShadeColor": [0.25, 0.25, 0.5, 1.0] }),
                    json!({}),
                ),
                json!({ "name": "Lashes", "shader": "VRM/UnlitCutout",
                        "floatProperties": { "_Cutoff": 0.4 }, "textureProperties": { "_MainTex": image::ALBEDO } }),
                json!({ "name": "Plain", "shader": "VRM_USE_GLTFSHADER" }),
                mtoon("Unused", json!({}), json!({}), json!({ "_MainTex": image::ALBEDO })),
            ]
        }),
    );
    finish(writer)
}

/// The VRM 1.0 materials.
fn materials_v1() -> Vec<Value> {
    let skin_mtoon = json!({
        "specVersion": "1.0",
        "shadeColorFactor": [0.4, 0.3, 0.5],
        "shadeMultiplyTexture": { "index": image::ALBEDO },
        "shadingShiftFactor": -0.1,
        "shadingShiftTexture": { "index": image::SHIFT, "scale": 0.5 },
        "shadingToonyFactor": 0.8,
        "giEqualizationFactor": 0.9,
        "matcapFactor": [0.5, 0.4, 0.5],
        "matcapTexture": { "index": image::MATCAP },
        "parametricRimColorFactor": [0.4, 0.2, 0.2],
        "rimMultiplyTexture": { "index": image::RIM },
        "rimLightingMixFactor": 0.5,
        "parametricRimFresnelPowerFactor": 2.0,
        "parametricRimLiftFactor": 0.1,
        "outlineWidthMode": "worldCoordinates",
        "outlineWidthFactor": 0.002,
        "outlineWidthMultiplyTexture": { "index": image::OUTLINE },
        "outlineColorFactor": [0.1, 0.05, 0.05],
        "outlineLightingMixFactor": 1.0,
        "uvAnimationMaskTexture": { "index": image::RIM },
        "uvAnimationScrollXSpeedFactor": 0.1
    });
    vec![
        json!({
            "name": "Skin",
            "pbrMetallicRoughness": { "baseColorFactor": [0.8, 0.6, 1.0, 1.0], "baseColorTexture": { "index": image::ALBEDO } },
            "normalTexture": { "index": image::NORMAL, "scale": 0.7 },
            "extensions": { "VRMC_materials_mtoon": skin_mtoon }
        }),
        json!({
            "name": "Hair",
            "pbrMetallicRoughness": { "baseColorFactor": [0.2, 0.2, 0.2, 1.0] },
            "alphaMode": "MASK",
            "alphaCutoff": 0.3,
            "extensions": { "VRMC_materials_mtoon": {
                "specVersion": "1.0",
                "shadeColorFactor": [0.05, 0.05, 0.2],
                "outlineWidthMode": "screenCoordinates",
                "outlineWidthFactor": 0.004
            } }
        }),
        json!({
            "name": "Lashes",
            "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0], "baseColorTexture": { "index": image::ALBEDO } },
            "alphaMode": "MASK",
            "alphaCutoff": 0.4,
            "extensions": { "KHR_materials_unlit": {} }
        }),
        json!({ "name": "Plain", "pbrMetallicRoughness": { "baseColorFactor": [0.3, 0.6, 0.3, 1.0] } }),
        json!({
            "name": "Unused",
            "pbrMetallicRoughness": { "baseColorTexture": { "index": image::ALBEDO } },
            "extensions": { "VRMC_materials_mtoon": { "specVersion": "1.0" } }
        }),
    ]
}

/// The glTF materials of the 0.x export: UniVRM 0.x writes MToon materials as unlit glTF
/// materials, which the material properties override.
fn materials_v0() -> Vec<Value> {
    let unlit = |name: &str, texture: Option<usize>| {
        let mut value = json!({ "name": name, "extensions": { "KHR_materials_unlit": {} },
                                 "pbrMetallicRoughness": { "baseColorFactor": [1.0, 1.0, 1.0, 1.0] } });
        if let Some(texture) = texture {
            value["pbrMetallicRoughness"]["baseColorTexture"] = json!({ "index": texture });
        }
        value
    };
    let mut lashes = unlit("Lashes", Some(image::ALBEDO));
    lashes["alphaMode"] = "MASK".into();
    lashes["alphaCutoff"] = 0.4.into();
    vec![
        unlit("Skin", Some(image::ALBEDO)),
        unlit("Hair", None),
        lashes,
        json!({ "name": "Plain", "pbrMetallicRoughness": { "baseColorFactor": [0.3, 0.6, 0.3, 1.0] } }),
        unlit("Unused", Some(image::ALBEDO)),
    ]
}

/// The scene, images and textures both versions share; `turned` lays it out facing −Z, as VRM
/// 0.x does.
fn scene(turned: bool) -> GltfWriter {
    let turn = |[x, y, z]: [f32; 3]| if turned { [-x, y, -z] } else { [x, y, z] };
    let mut writer = GltfWriter::default();

    // World translations facing +Z (every node only translates); the file's are these turned.
    let mut world = vec![Vec3::ZERO; NODES.len()];
    for (index, &(_, parent, translation)) in NODES.iter().enumerate() {
        world[index] = parent.map_or(Vec3::ZERO, |parent| world[parent]) + Vec3::from(translation);
    }

    // In `image`'s index order.
    let images = [
        png(image::SIZE, |x, y| [(x * 30) as u8, (y * 30) as u8, 200, 255 - (x * 10) as u8]),
        gray_png(|x, y| ((x + y) * 16) as u8),
        png(image::SIZE, |_, _| [128, 128, 255, 255]),
        png(image::SIZE, |x, _| [255, (x * 20) as u8, 0, 255]),
        png(image::SIZE, |x, _| [0, (x * 255 / (image::SIZE - 1)) as u8, 0, 255]),
        jpeg(),
        png(image::SIZE, |x, _| [(x * 255 / (image::SIZE - 1)) as u8, 0, 0, 255]),
    ];
    let names =
        ["body_albedo", "body_shift", "body_normal", "body_rim", "body_outline", "matcap", "body_outline_r"];
    let clamp = writer.add_sampler(json!({ "wrapS": 33071, "wrapT": 33071 }));
    for (index, (bytes, name)) in images.iter().zip(names).enumerate() {
        let mime = if index == image::MATCAP { "image/jpeg" } else { "image/png" };
        let image = writer.add_image(name, mime, bytes);
        let mut texture = json!({ "source": image });
        if index == image::OUTLINE || index == image::OUTLINE_R {
            texture["sampler"] = clamp.into();
        }
        assert_eq!(writer.add_texture(texture), index);
    }

    let joints = skin_joints();
    let joint = |node: usize| joints.iter().position(|&joint| joint == node).expect("a skin joint") as u16;
    let matrices: Vec<[f32; 16]> = joints
        .iter()
        .map(|&node| glam::Mat4::from_translation(-Vec3::from(turn(world[node].to_array()))).to_cols_array())
        .collect();

    // Body: a quad at the hips (Skin), one by the hair (Hair) and a triangle on the spine
    // (Plain), each bound wholly to one joint.
    let quad = |center: Vec3, half: f32, joint: u16| {
        let corners = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
        Geometry {
            positions: corners
                .iter()
                .map(|[x, y]| turn((center + Vec3::new(x * half, y * half, 0.05)).to_array()))
                .collect(),
            normals: vec![turn([0.0, 0.0, 1.0]); 4],
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            colors: vec![[10, 20, 30, 40]; 4],
            influences: vec![InfluenceSet {
                joints: vec![[joint, 0, 0, 0]; 4],
                weights: vec![[1.0, 0.0, 0.0, 0.0]; 4],
            }],
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Geometry::default()
        }
    };
    let skin_quad = quad(Vec3::new(0.0, 0.95, 0.0), 0.1, joint(node::HIPS));
    let hair_quad = quad(world[node::HAIR2], 0.05, joint(node::HAIR2));
    let mut plain = quad(Vec3::new(0.0, 1.05, 0.0), 0.05, joint(node::SPINE));
    plain.indices.truncate(3);
    let body = writer.add_mesh(
        "Body",
        &[
            (&skin_quad, Some(material::SKIN)),
            (&hair_quad, Some(material::HAIR)),
            (&plain, Some(material::PLAIN)),
        ],
    );

    // Face: a static quad by the head with `blink` (top corners down) and `smile` (one corner up).
    let mut face = quad(Vec3::new(0.0, 1.45, 0.05), 0.08, 0);
    face.influences.clear();
    face.targets = vec![
        MorphGeometry {
            positions: vec![[0.0; 3], [0.0; 3], [0.0, -0.01, 0.0], [0.0, -0.01, 0.0]],
            ..MorphGeometry::default()
        },
        MorphGeometry {
            positions: vec![[0.0, 0.01, 0.0], [0.0; 3], [0.0; 3], [0.0; 3]],
            ..MorphGeometry::default()
        },
    ];
    let face_mesh = writer.add_mesh("Face", &[(&face, Some(material::SKIN))]);
    writer.set_mesh_field(face_mesh, "extras", json!({ "targetNames": ["blink", "smile"] }));

    let mut lashes = quad(Vec3::new(0.0, 1.5, 0.06), 0.05, 0);
    lashes.influences.clear();
    lashes.indices.truncate(3);
    lashes.targets =
        vec![MorphGeometry { positions: vec![[0.0, -0.005, 0.0]; 4], ..MorphGeometry::default() }];
    let lashes_mesh = writer.add_mesh("Lashes", &[(&lashes, Some(material::LASHES))]);
    writer.set_mesh_field(lashes_mesh, "extras", json!({ "targetNames": ["blink"] }));

    let skin = writer.add_skin(&joints, Some(&matrices));
    for (index, &(name, _, translation)) in NODES.iter().enumerate() {
        let children: Vec<usize> = NODES
            .iter()
            .enumerate()
            .filter(|(_, (_, parent, _))| *parent == Some(index))
            .map(|(child, _)| child)
            .collect();
        let mut value = json!({ "name": name, "translation": turn(translation) });
        if !children.is_empty() {
            value["children"] = children.into();
        }
        match index {
            node::BODY => {
                value["mesh"] = body.into();
                value["skin"] = skin.into();
            }
            node::FACE => value["mesh"] = face_mesh.into(),
            node::LASHES => value["mesh"] = lashes_mesh.into(),
            node::TWIST if !turned => {
                value["extensions"] = json!({ "VRMC_node_constraint": {
                    "specVersion": "1.0",
                    "constraint": { "roll": { "source": node::LEFT_HAND, "rollAxis": "X", "weight": 0.5 } }
                } });
            }
            _ => {}
        }
        assert_eq!(writer.add_node(value), index);
    }
    writer.add_root(node::ROOT);
    writer
}

fn finish(writer: GltfWriter) -> SampleVrm {
    SampleVrm { document: writer.document(None), bin: writer.bin().to_vec() }
}

/// An 8-bit RGBA PNG.
pub(crate) fn png(size: u32, texel: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
    let image = ::image::RgbaImage::from_fn(size, size, |x, y| ::image::Rgba(texel(x, y)));
    let mut bytes = Vec::new();
    image.write_to(&mut Cursor::new(&mut bytes), ::image::ImageFormat::Png).expect("encode a PNG");
    bytes
}

fn gray_png(texel: impl Fn(u32, u32) -> u8) -> Vec<u8> {
    let image = ::image::GrayImage::from_fn(image::SIZE, image::SIZE, |x, y| ::image::Luma([texel(x, y)]));
    let mut bytes = Vec::new();
    image.write_to(&mut Cursor::new(&mut bytes), ::image::ImageFormat::Png).expect("encode a PNG");
    bytes
}

fn jpeg() -> Vec<u8> {
    let image = ::image::RgbImage::from_fn(image::SIZE, image::SIZE, |x, y| {
        ::image::Rgb([(x * 32) as u8, 128, (y * 32) as u8])
    });
    let mut bytes = Vec::new();
    image.write_to(&mut Cursor::new(&mut bytes), ::image::ImageFormat::Jpeg).expect("encode a JPEG");
    bytes
}
