use super::*;
use crate::material::{AnimeCharacterMaterial, Material};
use crate::mesh::{
    Influences, Joint, JointTransform, MaterialSlot, Mesh, MorphTarget, PositionFormat, Skin, SkinBind,
    Submesh, Vertex, VertexInfluence,
};
use crate::texture::Swizzle;

/// A humanoid whose required bones are joints 0 to 14, in [`HumanBone::REQUIRED`]'s order
/// sorted by bone.
fn humanoid() -> Vec<HumanoidJoint> {
    let mut bones = HumanBone::REQUIRED.to_vec();
    bones.sort();
    bones.into_iter().enumerate().map(|(joint, bone)| HumanoidJoint { bone, joint: joint as u16 }).collect()
}

/// A rig for a mesh of 20 joints, 3 morph targets and 2 material slots, using every part of
/// the format.
fn rig() -> CharacterRig {
    let joint = |joint: u16| SpringJoint {
        joint,
        hit_radius: 0.02,
        stiffness: 1.0,
        gravity_power: 0.0,
        gravity_dir: [0.0, -1.0, 0.0],
        drag_force: 0.5,
    };
    CharacterRig {
        joint_count: 20,
        morph_target_count: 3,
        material_slot_count: 2,
        humanoid: humanoid(),
        expressions: vec![
            Expression {
                name: "blink".into(),
                preset: Some(ExpressionPreset::Blink),
                binary: false,
                override_blink: ExpressionOverride::None,
                override_look_at: ExpressionOverride::Block,
                override_mouth: ExpressionOverride::Blend,
                morph_targets: vec![
                    MorphTargetWeight { target: 0, weight: 1.0 },
                    MorphTargetWeight { target: 2, weight: 0.5 },
                ],
                material_colors: vec![MaterialColorBind {
                    slot: 1,
                    color: MaterialColor::Shade,
                    value: [1.0, 0.5, 0.5, 1.0],
                }],
                texture_transforms: vec![TextureTransformBind {
                    slot: 0,
                    scale: [1.0; 2],
                    offset: [0.25, 0.0],
                }],
            },
            Expression {
                name: "wink sparkle".into(),
                preset: None,
                binary: true,
                override_blink: ExpressionOverride::Block,
                override_look_at: ExpressionOverride::None,
                override_mouth: ExpressionOverride::None,
                morph_targets: vec![MorphTargetWeight { target: 1, weight: 1.0 }],
                material_colors: Vec::new(),
                texture_transforms: Vec::new(),
            },
        ],
        look_at: Some(LookAt {
            offset_from_head: [0.0, 0.06, 0.02],
            kind: LookAtKind::Bone,
            horizontal_inner: LookAtRange { input_max_degrees: 90.0, output_scale: 10.0 },
            horizontal_outer: LookAtRange { input_max_degrees: 90.0, output_scale: 10.0 },
            vertical_down: LookAtRange { input_max_degrees: 90.0, output_scale: 8.0 },
            vertical_up: LookAtRange { input_max_degrees: 90.0, output_scale: 8.0 },
        }),
        springs: vec![
            Spring {
                name: "hair".into(),
                joints: vec![joint(15), joint(16), joint(17)],
                tip: SpringTip::Joint,
                collider_groups: vec![0],
                center: Some(0),
            },
            Spring {
                name: "ribbon".into(),
                joints: vec![joint(18)],
                tip: SpringTip::Extended { length: 0.07 },
                collider_groups: Vec::new(),
                center: None,
            },
        ],
        colliders: vec![
            Collider { joint: 2, shape: ColliderShape::Sphere { offset: [0.0, 0.1, 0.0], radius: 0.1 } },
            Collider {
                joint: 1,
                shape: ColliderShape::Capsule { offset: [0.0; 3], radius: 0.08, tail: [0.0, 0.2, 0.0] },
            },
        ],
        collider_groups: vec![ColliderGroup { name: "head".into(), colliders: vec![0, 1] }],
        constraints: vec![
            Constraint { joint: 19, source: 10, weight: 0.5, kind: ConstraintKind::Roll { axis: Axis::X } },
            Constraint {
                joint: 18,
                source: 19,
                weight: 1.0,
                kind: ConstraintKind::Aim { axis: AimAxis::NegativeZ },
            },
            Constraint { joint: 17, source: 3, weight: 1.0, kind: ConstraintKind::Rotation },
        ],
    }
}

fn cook(rig: &CharacterRig) -> AlignedVec {
    rig.to_cooked().expect("a rig always archives")
}

/// The error reading `rig` back gives, after `edit` breaks it.
fn error_after(edit: impl FnOnce(&mut CharacterRig)) -> RigError {
    let mut broken = rig();
    edit(&mut broken);
    CharacterRig::read(&cook(&broken)).expect_err("the edit breaks an invariant")
}

#[test]
fn rigs_round_trip_through_cooked_bytes() {
    let rig = rig();
    let bytes = cook(&rig);
    assert_eq!(CharacterRig::from_cooked(&bytes).unwrap(), rig);
    let archived = CharacterRig::read(&bytes).unwrap();
    assert_eq!(archived.springs[0].joints.len(), 3);
    assert!(matches!(archived.springs[1].tip, ArchivedSpringTip::Extended { .. }));
    assert_eq!(rig.human_joint(HumanBone::Hips), Some(0));
    assert_eq!(rig.human_joint(HumanBone::Jaw), None);
    assert_eq!(rig.expression("blink").and_then(|e| e.preset), Some(ExpressionPreset::Blink));
    // An empty rig (a mesh without VRM data) is valid too.
    CharacterRig::read(&cook(&CharacterRig::default())).unwrap();
}

#[test]
fn indices_must_be_in_range() {
    assert!(matches!(error_after(|r| r.humanoid[0].joint = 20), RigError::Joint { joint: 20, .. }));
    assert!(matches!(error_after(|r| r.springs[0].joints[1].joint = 99), RigError::Joint { joint: 99, .. }));
    assert!(matches!(error_after(|r| r.springs[0].center = Some(20)), RigError::Joint { .. }));
    assert!(matches!(error_after(|r| r.colliders[1].joint = 20), RigError::Joint { .. }));
    assert!(matches!(error_after(|r| r.constraints[0].source = 20), RigError::Joint { .. }));
    let error = error_after(|r| r.expressions[0].morph_targets[1].target = 3);
    assert!(matches!(error, RigError::MorphTarget { target: 3, count: 3, .. }), "{error}");
    let error = error_after(|r| r.expressions[0].material_colors[0].slot = 2);
    assert!(matches!(error, RigError::MaterialSlot { slot: 2, count: 2, .. }), "{error}");
    assert!(matches!(
        error_after(|r| r.expressions[0].texture_transforms[0].slot = 7),
        RigError::MaterialSlot { .. }
    ));
    assert!(matches!(
        error_after(|r| r.springs[0].collider_groups = vec![1]),
        RigError::Index { index: 1, .. }
    ));
    assert!(matches!(
        error_after(|r| r.collider_groups[0].colliders.push(2)),
        RigError::Index { index: 2, .. }
    ));
}

#[test]
fn the_humanoid_is_sorted_unique_and_complete() {
    let error = error_after(|r| r.humanoid.swap(0, 1));
    assert!(error.to_string().contains("out of order"), "{error}");
    let error = error_after(|r| r.humanoid[1].joint = r.humanoid[0].joint);
    assert!(error.to_string().contains("shares its joint"), "{error}");
    let error = error_after(|r| r.humanoid.retain(|entry| entry.bone != HumanBone::LeftFoot));
    assert!(matches!(error, RigError::MissingBone(HumanBone::LeftFoot)), "{error}");
    // Optional bones may join.
    let mut rig = rig();
    rig.humanoid.push(HumanoidJoint { bone: HumanBone::RightLittleDistal, joint: 19 });
    CharacterRig::read(&cook(&rig)).unwrap();
}

#[test]
fn expressions_have_unique_names_and_ordered_weights() {
    let error = error_after(|r| r.expressions[1].name = "blink".into());
    assert!(error.to_string().contains("repeated name"), "{error}");
    let error = error_after(|r| r.expressions[0].name = "closed".into());
    assert!(error.to_string().contains("named after its preset"), "{error}");
    let error = error_after(|r| r.expressions[0].morph_targets.swap(0, 1));
    assert!(error.to_string().contains("out of order"), "{error}");
    let error = error_after(|r| r.expressions[0].morph_targets[0].weight = 1.5);
    assert!(error.to_string().contains("between 0 and 1"), "{error}");
    let error = error_after(|r| r.expressions[0].material_colors[0].value[0] = f32::NAN);
    assert!(error.to_string().contains("material color"), "{error}");
    let error = error_after(|r| r.expressions[0].texture_transforms[0].offset[1] = f32::INFINITY);
    assert!(error.to_string().contains("UV offset"), "{error}");
}

#[test]
fn numbers_are_finite_and_in_range() {
    let error = error_after(|r| r.springs[0].joints[0].drag_force = 1.5);
    assert!(error.to_string().contains("drag force"), "{error}");
    let error = error_after(|r| r.springs[0].joints[2].hit_radius = -0.1);
    assert!(error.to_string().contains("hit radius"), "{error}");
    let error = error_after(|r| r.springs[0].joints[0].gravity_dir[2] = f32::NAN);
    assert!(error.to_string().contains("gravity direction"), "{error}");
    let error = error_after(|r| r.springs[0].joints[1].gravity_dir = [0.0, -2.0, 0.0]);
    assert!(error.to_string().contains("unit vector"), "{error}");
    let error = error_after(|r| r.springs[1].tip = SpringTip::Extended { length: 0.0 });
    assert!(error.to_string().contains("tip length"), "{error}");
    let error = error_after(|r| r.springs[1].joints.clear());
    assert!(error.to_string().contains("no joints"), "{error}");
    let error = error_after(|r| {
        r.colliders[0].shape = ColliderShape::Sphere { offset: [0.0; 3], radius: f32::NAN };
    });
    assert!(error.to_string().contains("collider 0 radius"), "{error}");
    let error = error_after(|r| r.look_at.as_mut().unwrap().vertical_up.output_scale = f32::INFINITY);
    assert!(error.to_string().contains("look-at vertical up"), "{error}");
    let error = error_after(|r| r.constraints[1].weight = -0.5);
    assert!(error.to_string().contains("weight"), "{error}");
}

#[test]
fn constraints_never_follow_themselves() {
    let error = error_after(|r| r.constraints[0].source = 19);
    assert!(error.to_string().contains("its own joint"), "{error}");
    // 19 follows 10, 18 follows 19; closing the loop with 10 following 18 is a cycle.
    let error = error_after(|r| {
        r.constraints.push(Constraint { joint: 10, source: 18, weight: 1.0, kind: ConstraintKind::Rotation });
    });
    assert!(error.to_string().contains("cycle"), "{error}");
    // A joint has one constraint, so a second on 19 is refused rather than hiding a cycle
    // behind the first.
    let error = error_after(|r| {
        r.constraints.push(Constraint { joint: 19, source: 18, weight: 1.0, kind: ConstraintKind::Rotation });
    });
    assert!(error.to_string().contains("another constraint"), "{error}");
}

/// A one-triangle mesh with `joints` joints, `targets` morph targets and `slots` slots.
fn mesh(joints: usize, targets: usize, slots: usize) -> AlignedVec {
    let vertex = Vertex { normal: [0.0, 0.0, 1.0], outline_normal: [0.0, 0.0, 1.0], ..Vertex::default() };
    let swizzles = [Swizzle::IDENTITY; AnimeCharacterMaterial::TEXTURE_SLOTS.len()];
    let material = Material::from_document(&AnimeCharacterMaterial::default(), &swizzles).unwrap();
    let materials =
        (0..slots).map(|slot| MaterialSlot::embedded(format!("#{slot}"), material.clone())).collect();
    let submeshes = vec![Submesh { first_index: 0, index_count: 3, material: 0 }];
    let mut mesh =
        Mesh::from_vertices(&[vertex; 3], vec![0, 1, 2], submeshes, materials, PositionFormat::Float32);
    if joints > 0 {
        let identity =
            [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];
        mesh.skin = Some(Skin {
            joints: (0..joints)
                .map(|joint| Joint {
                    name: format!("j{joint}"),
                    parent: joint.checked_sub(1).map(|parent| parent as u16),
                    rest: JointTransform::IDENTITY,
                })
                .collect(),
            binds: vec![SkinBind { joint: 0, inverse_bind_matrix: identity }],
            influences: Influences::pack(&[VertexInfluence { binds: [0; 4], weights: [255, 0, 0, 0] }; 3], 1),
        });
    }
    mesh.morph_targets = (0..targets)
        .map(|target| MorphTarget {
            name: format!("t{target}"),
            default_weight: 0.0,
            first_delta: 0,
            delta_count: 0,
        })
        .collect();
    mesh.to_cooked().unwrap()
}

#[test]
fn a_rig_pairs_only_with_a_mesh_of_its_shape() {
    let bytes = cook(&rig());
    let rig = CharacterRig::read(&bytes).unwrap();
    let fits = mesh(20, 3, 2);
    rig.check_mesh(Mesh::read(&fits).unwrap()).unwrap();
    for (joints, targets, slots) in [(19, 3, 2), (20, 2, 2), (20, 3, 3), (0, 3, 2)] {
        let other = mesh(joints, targets, slots);
        let error = rig.check_mesh(Mesh::read(&other).unwrap()).unwrap_err();
        assert!(matches!(error, RigError::MeshMismatch { .. }), "{error}");
        assert!(error.to_string().contains("20 joints, 3 morph targets and 2 material slots"), "{error}");
    }
}

#[test]
fn names_follow_vrm_1_0() {
    assert_eq!(HumanBone::ALL.len(), 55);
    let mut names: Vec<&str> = HumanBone::ALL.iter().map(|bone| bone.name()).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 55, "unique names");
    for &bone in HumanBone::ALL {
        assert_eq!(HumanBone::from_name(bone.name()), Some(bone));
    }
    assert_eq!(HumanBone::from_name("leftThumbIntermediate"), None, "a VRM 0.x name");
    assert!(HumanBone::REQUIRED.iter().all(|bone| HumanBone::ALL.contains(bone)));
    assert_eq!(ExpressionPreset::ALL.len(), 18);
    assert_eq!(ExpressionPreset::from_name("blinkLeft"), Some(ExpressionPreset::BlinkLeft));
    assert_eq!(ExpressionPreset::from_name("joy"), None, "a VRM 0.x name");
    assert_eq!(AimAxis::from_name("NegativeY"), Some(AimAxis::NegativeY));
    assert_eq!(MaterialColor::from_name("shadeColor"), Some(MaterialColor::Shade));
}
