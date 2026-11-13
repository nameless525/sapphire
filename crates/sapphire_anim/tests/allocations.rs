//! Evaluating a pose allocates nothing once it is made: sampling, blending, additive layers,
//! constraints, local-to-model, the palette and spring bones all run every frame for every character.
//! Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;

use sapphire_anim::{
    CharacterPose, Clip, ClothSheet, FeetInput, FootGround, Grip, GroundHit, Hand, HeldWeapon, IkInput,
    Joint, JointMask, JointTransform, LookInput, PackedRotation, RotationTrack, Skeleton, SpringBones,
    SpringFrame, Substeps, Transform, VectorRange, VectorTrack, WeaponPlace, add, blend, local_to_model,
    make_delta, root_motion, sample,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{DEFAULT_RATE, RootMotion};
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::{
    AimAxis, Axis, CharacterRig, Collider as RigCollider, ColliderGroup, ColliderShape, Constraint,
    ConstraintKind, Spring, SpringJoint, SpringTip,
};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};

mod humanoid;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

const JOINTS: usize = 64;

/// A chain of joints, each 10 cm along X from its parent.
fn skeleton() -> Skeleton {
    let joints: Vec<Joint> = (0..JOINTS)
        .map(|joint| Joint {
            name: format!("joint {joint}"),
            parent: joint.checked_sub(1).map(|parent| parent as u16),
            rest: JointTransform { translation: [0.1, 0.0, 0.0], ..JointTransform::IDENTITY },
        })
        .collect();
    Skeleton::new(&joints)
}

/// Every joint turning over 60 ticks, one moving, with root motion.
fn clip() -> Clip {
    let turn = Quat::from_rotation_z(0.2).to_array();
    let moved = VectorRange::enclosing([[0.1, 0.0, 0.0], [0.1, 0.2, 0.0]]);
    let root = VectorRange::enclosing([[0.0; 3], [0.0, 0.0, 1.0]]);
    let rotations: Vec<RotationTrack> = (0..JOINTS)
        .map(|joint| RotationTrack { joint: joint as u16, count: 2, first: 2 * joint as u32 })
        .collect();
    let mut rotation_ticks = Vec::new();
    let mut rotation_keys = Vec::new();
    for _ in 0..JOINTS {
        rotation_ticks.extend([0, 60]);
        rotation_keys.extend([PackedRotation::encode([0.0, 0.0, 0.0, 1.0]), PackedRotation::encode(turn)]);
    }
    rotation_ticks.push(0);
    rotation_keys.push(PackedRotation::encode([0.0, 0.0, 0.0, 1.0]));
    Clip {
        name: "chain".into(),
        skeleton: Some(AssetId::from_bytes([3; 16])),
        joint_count: JOINTS as u32,
        rate: DEFAULT_RATE,
        ticks: 60,
        rotations,
        translations: vec![VectorTrack { joint: 3, count: 2, first: 0, range: moved }],
        root: Some(RootMotion {
            joint: 0,
            translation: VectorTrack { joint: 0, count: 2, first: 2, range: root },
            rotation: RotationTrack { joint: 0, count: 1, first: 2 * JOINTS as u32 },
        }),
        rotation_ticks,
        rotation_keys,
        vector_ticks: vec![0, 60, 0, 60],
        vector_keys: vec![
            moved.encode([0.1, 0.0, 0.0]),
            moved.encode([0.1, 0.2, 0.0]),
            root.encode([0.0; 3]),
            root.encode([0.0, 0.0, 1.0]),
        ],
        ..Clip::default()
    }
}

#[test]
fn evaluating_a_pose_allocates_nothing_once_it_is_made() {
    let skeleton_bytes = skeleton().to_cooked().expect("cooked");
    let cooked = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let clip_bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&clip_bytes).expect("a valid clip");
    let binds: Vec<SkinBind> = (0..JOINTS)
        .map(|joint| SkinBind {
            joint: joint as u16,
            inverse_bind_matrix: Mat4::from_translation(Vec3::NEG_X).to_cols_array_2d(),
        })
        .collect();
    // An aim, whose chain walks most of the skeleton, and a roll.
    let constraints = [
        Constraint {
            joint: 60,
            source: 40,
            weight: 1.0,
            kind: ConstraintKind::Aim { axis: AimAxis::PositiveX },
        },
        Constraint { joint: 41, source: 40, weight: 0.5, kind: ConstraintKind::Roll { axis: Axis::X } },
    ];
    let mut pose = CharacterPose::new(cooked, &binds, &constraints, vec![0.0; 4]);
    let rest = pose.rest().to_vec();
    let mut other = rest.clone();
    let mut delta = rest.clone();
    let mut model = vec![Affine3A::IDENTITY; JOINTS];
    let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
    let mut mask = JointMask::everything(JOINTS);
    mask.set(0, 0.25);

    let (sum, counts) = alloc::count_in_scope(|| {
        let mut sum = 0.0;
        for frame in 0..600_u32 {
            let position = (frame % 120) as f32 * 0.5;
            pose.sample(clip, position);
            sample(clip, position + 3.0, &rest, &mut other);
            blend(pose.locals_mut(), &other, 0.5, None);
            blend(pose.locals_mut(), &other, 0.5, Some(&mask));
            make_delta(&other, &rest, &mut delta);
            add(pose.locals_mut(), &delta, 0.25, Some(&mask));
            pose.finish();
            pose.write_palette(&mut palette);
            local_to_model(cooked, pose.locals(), &mut model);
            let root = root_motion(clip, position).expect("it has root motion");
            sum += root.translation.z + palette[JOINTS - 1].translation.x + model[1].translation.y;
        }
        sum
    });
    assert!(sum.is_finite());
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
    assert_eq!(pose.locals().len(), JOINTS);
}

/// Spring bones allocate nothing once made, through everything a frame can ask of them: sub-steps on
/// both sub-step counts that ship, colliders, a reset, sleeping and waking.
#[test]
fn updating_springs_allocates_nothing_once_they_are_made() {
    let skeleton_bytes = skeleton().to_cooked().expect("cooked");
    let cooked = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let clip_bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&clip_bytes).expect("a valid clip");
    let joint = |joint: u16| SpringJoint {
        joint,
        hit_radius: 0.02,
        stiffness: 0.5,
        gravity_power: 0.1,
        gravity_dir: [0.0, -1.0, 0.0],
        drag_force: 0.4,
    };
    let chain = |first: u16, center: Option<u16>| Spring {
        name: "chain".into(),
        joints: (first..first + 6).map(joint).collect(),
        tip: SpringTip::Joint,
        collider_groups: vec![0],
        center,
    };
    let rig = CharacterRig {
        joint_count: JOINTS as u32,
        springs: vec![chain(10, None), chain(20, Some(2)), chain(30, None)],
        colliders: vec![
            RigCollider { joint: 2, shape: ColliderShape::Sphere { offset: [0.0; 3], radius: 0.1 } },
            RigCollider {
                joint: 4,
                shape: ColliderShape::Capsule { offset: [0.0; 3], radius: 0.05, tail: [0.1, 0.0, 0.0] },
            },
        ],
        collider_groups: vec![ColliderGroup { name: "body".into(), colliders: vec![0, 1] }],
        ..CharacterRig::default()
    };
    let rig_bytes = rig.to_cooked().expect("cooked");
    let rig = CharacterRig::read(&rig_bytes).expect("a valid rig");
    let mut pose = CharacterPose::new(cooked, &[], &[], Vec::new());
    let mut springs = SpringBones::new(rig, &pose).expect("it has springs");

    let (sum, counts) = alloc::count_in_scope(|| {
        let mut sum = 0.0;
        for frame in 0..1_200_u32 {
            if frame % 300 == 100 {
                springs.reset();
            }
            if frame == 600 {
                springs.set_substeps(Substeps::One);
            }
            let moved = frame % 400 < 300;
            if moved {
                pose.sample(clip, (frame % 60) as f32);
                pose.finish();
            }
            let world = Affine3A::from_translation(Vec3::new(
                f64::from(frame).sin() as f32,
                0.0,
                frame as f32 * 0.03,
            ));
            let active = frame % 200 < 150;
            let work = springs.update(
                &mut pose,
                &SpringFrame { world_from_model: world, ticks: f64::from(frame), pose_moved: moved, active },
            );
            sum += work.substeps as f32 + pose.model()[15].translation.y;
        }
        sum
    });
    assert!(sum.is_finite());
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
}

/// Cloth allocates nothing once its sheets are set, through everything a frame and a tier can ask of it:
/// its iterations changing, off and on again, sub-step counts, colliders, a reset, sleeping and waking,
/// and a new version of the character taking its motion over.
#[test]
fn updating_cloth_allocates_nothing_once_it_is_made() {
    let skeleton_bytes = skeleton().to_cooked().expect("cooked");
    let cooked = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let clip_bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&clip_bytes).expect("a valid clip");
    let joint = |joint: u16| SpringJoint {
        joint,
        hit_radius: 0.02,
        stiffness: 0.3,
        gravity_power: 0.5,
        gravity_dir: [0.0, -1.0, 0.0],
        drag_force: 0.2,
    };
    // Four chains, each hanging below the last, so the cloth walks again what hangs below it too.
    let chain = |name: &str, first: u16| Spring {
        name: name.into(),
        joints: (first..first + 6).map(joint).collect(),
        tip: SpringTip::Extended { length: 0.05 },
        collider_groups: vec![0],
        center: None,
    };
    let rig = CharacterRig {
        joint_count: JOINTS as u32,
        springs: vec![chain("a", 10), chain("b", 20), chain("c", 30), chain("d", 40)],
        colliders: vec![
            RigCollider { joint: 2, shape: ColliderShape::Sphere { offset: [0.0; 3], radius: 0.1 } },
            RigCollider {
                joint: 4,
                shape: ColliderShape::Capsule { offset: [0.0; 3], radius: 0.05, tail: [0.1, 0.0, 0.0] },
            },
        ],
        collider_groups: vec![ColliderGroup { name: "body".into(), colliders: vec![0, 1] }],
        ..CharacterRig::default()
    };
    let rig_bytes = rig.to_cooked().expect("cooked");
    let rig = CharacterRig::read(&rig_bytes).expect("a valid rig");
    let mut pose = CharacterPose::new(cooked, &[], &[], Vec::new());
    let sheet = ClothSheet {
        chains: ["a", "b", "c"].map(String::from).to_vec(),
        closed: true,
        lateral_compliance: 1e-6,
        bend_compliance: 1e-4,
    };
    let make = || {
        let mut springs = SpringBones::new(rig, &pose).expect("it has springs");
        springs.set_cloth(rig, std::slice::from_ref(&sheet)).expect("its chains");
        springs
    };
    let mut springs = make();
    let mut next = make();

    let (sum, counts) = alloc::count_in_scope(|| {
        let mut sum = 0.0;
        for frame in 0..1_200_u32 {
            if frame % 300 == 100 {
                springs.reset();
            }
            if frame == 600 {
                springs.set_substeps(Substeps::One);
            }
            if frame == 700 {
                next.continue_from(&springs);
                std::mem::swap(&mut springs, &mut next);
            }
            springs.set_cloth_iterations([4, 2, 1, 0][(frame / 50 % 4) as usize]);
            let moved = frame % 400 < 300;
            if moved {
                pose.sample(clip, (frame % 60) as f32);
                pose.finish();
            }
            let world = Affine3A::from_translation(Vec3::new(
                f64::from(frame).sin() as f32,
                0.0,
                frame as f32 * 0.03,
            ));
            let active = frame % 200 < 150;
            let work = springs.update(
                &mut pose,
                &SpringFrame { world_from_model: world, ticks: f64::from(frame), pose_moved: moved, active },
            );
            sum += work.substeps as f32 + pose.model()[15].translation.y + springs.cloth_strain();
        }
        sum
    });
    assert!(sum.is_finite());
    assert_eq!(springs.cloth_iterations(), Some(0));
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
}

/// Every stage of IK at once, with a moving target and changing ground, frame after frame.
#[test]
fn solving_ik_allocates_nothing_once_it_is_made() {
    let (skeleton, bones) = humanoid::humanoid(100);
    let bytes = skeleton.to_cooked().expect("cooked");
    let cooked = Skeleton::read(&bytes).expect("a valid skeleton");
    let mut pose = CharacterPose::new(cooked, &[], &[], Vec::new());
    let mut ik = humanoid::ik(&pose, &bones);
    let world = Affine3A::from_translation(Vec3::new(1.0, 0.2, -1.0));
    let weapon = HeldWeapon {
        place: WeaponPlace::Joint { joint: 19, offset: Affine3A::from_translation(Vec3::X * 0.1) },
        main: Hand::Right,
        main_grip: None,
        off_grip: Some(Grip {
            on_weapon: Transform::from_translation(Vec3::new(-0.3, 0.0, 0.1)),
            in_hand: Transform::from_translation(Vec3::X * 0.05),
            pole: Some(Vec3::NEG_Y),
            weight: 1.0,
        }),
    };
    let (sum, counts) = alloc::count_in_scope(|| {
        let mut sum = 0.0;
        for frame in 0..600_u32 {
            let time = frame as f32 / 60.0;
            pose.reset();
            ik.prepare(&mut pose);
            let probes = ik.foot_probes(&mut pose, &world).expect("it has feet");
            let tilt = Vec3::new((time * 2.0).sin() * 0.4, 1.0, 0.0).normalize();
            let hit = |at: Vec3| {
                Some(GroundHit { point: Vec3::new(at.x, 0.1 * (time * 3.0).sin(), at.z), normal: tilt })
            };
            let input = IkInput {
                dt: 1.0 / 60.0,
                world_from_model: world,
                feet: FeetInput {
                    weight: if frame % 200 < 150 { 1.0 } else { 0.0 },
                    ground: [
                        FootGround { ankle: hit(probes[0].ankle), ball: probes[0].ball.and_then(hit) },
                        FootGround { ankle: hit(probes[1].ankle), ball: probes[1].ball.and_then(hit) },
                    ],
                },
                look: LookInput {
                    target: Some(Vec3::new(4.0 * time.sin(), 1.5, 4.0 * time.cos())),
                    weight: if frame % 300 < 250 { 1.0 } else { 0.0 },
                },
                weapon: Some(weapon),
            };
            ik.solve(&mut pose, &input);
            pose.finish();
            sum += pose.model()[5].translation.y;
        }
        sum
    });
    assert!(sum.is_finite());
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
}

#[test]
fn a_face_allocates_nothing_once_it_is_bound() {
    use sapphire_anim::face::{FaceLayer, FaceOrder, Mouth};
    use sapphire_assets::mouth::{MouthCurve, MouthFrame};
    use sapphire_assets::rig::{Expression, ExpressionOverride, ExpressionPreset, MorphTargetWeight};
    use sapphire_core::Name;

    let expressions =
        [ExpressionPreset::Happy, ExpressionPreset::Angry, ExpressionPreset::Aa, ExpressionPreset::Blink]
            .into_iter()
            .enumerate()
            .map(|(target, preset)| Expression {
                name: preset.name().to_owned(),
                preset: Some(preset),
                binary: false,
                override_blink: ExpressionOverride::Blend,
                override_look_at: ExpressionOverride::None,
                override_mouth: ExpressionOverride::Block,
                morph_targets: vec![MorphTargetWeight { target: target as u32, weight: 1.0 }],
                material_colors: Vec::new(),
                texture_transforms: Vec::new(),
            })
            .collect();
    let bytes =
        CharacterRig { morph_target_count: 4, expressions, ..CharacterRig::default() }.to_cooked().unwrap();
    let rig = CharacterRig::read(&bytes).unwrap();
    let mut face = FaceLayer::new(rig, 4, Some((ExpressionPreset::Happy, 0.3))).unwrap();
    let names = [Name::new("happy"), Name::new("angry"), Name::new("aa"), Name::new("blink")];
    // A voice's cooked mouth, read at its playhead as a conversation's line drives it (M5 C6).
    let frames = (0..=120_u8).map(|frame| MouthFrame { open: frame * 2, visemes: [frame * 2, 0, 0, 0, 0] });
    let curve = MouthCurve { frames: frames.collect() };
    let mut weights = [0.0_f32; 4];
    let (sum, counts) = alloc::count_in_scope(|| {
        let mut sum = 0.0;
        for frame in 0..240_u64 {
            let key = frame / 40;
            if key % 3 == 2 {
                face.hand_back(0.2);
            } else {
                face.show(FaceOrder {
                    key,
                    expression: names[(key % 4) as usize],
                    weight: 1.0,
                    blend: 0.2,
                    since: 0.0,
                });
            }
            face.set_mouth(if frame < 120 {
                Mouth::said(curve.at(frame as f32 / 60.0))
            } else {
                Mouth::open((frame % 7) as f32 / 7.0)
            });
            face.advance(1.0 / 60.0);
            weights = [0.3, 0.0, 0.0, 0.5];
            face.apply(&mut weights);
            sum += weights.iter().sum::<f32>();
        }
        sum
    });
    assert!(sum.is_finite() && weights.iter().all(|weight| (0.0..=1.0).contains(weight)));
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
}
