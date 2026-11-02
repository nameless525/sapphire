//! Evaluating a pose allocates nothing once it is made: sampling, blending, additive layers,
//! constraints, local-to-model and the palette all run every frame for every character. Installs
//! `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;

use sapphire_anim::{
    CharacterPose, Clip, Joint, JointMask, JointTransform, PackedRotation, RotationTrack, Skeleton,
    VectorRange, VectorTrack, add, blend, local_to_model, make_delta, root_motion, sample,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::{DEFAULT_RATE, RootMotion};
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::{AimAxis, Axis, Constraint, ConstraintKind};
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};

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
