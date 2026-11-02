//! Playing a graph allocates nothing once its player is made: every layer, cancels, a state's start, a
//! reaction, a blink, a frozen clock, a cut and a replacement taking effect, for every character every
//! frame. Installs `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;
use std::sync::Arc;

use sapphire_anim::{
    AnimGraph, CharacterPose, Clip, CombatInput, GraphBinding, GraphInput, GraphPlayer, Joint,
    JointTransform, LocomotionInput, PackedRotation, ReactionInput, RotationTrack, Skeleton,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::DEFAULT_RATE;
use sapphire_assets::rig::{
    CharacterRig, Expression, ExpressionOverride, ExpressionPreset, HumanBone, HumanoidJoint,
    MorphTargetWeight,
};
use sapphire_core::Name;
use sapphire_core::alloc::{self, CountingAllocator, PER_THREAD_COUNTS};
use sapphire_math::Quat;

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

/// Every joint turning over 60 ticks.
fn clip() -> Clip {
    let turn = Quat::from_rotation_z(0.2).to_array();
    let mut clip = Clip {
        name: "chain".into(),
        skeleton: Some(AssetId::from_bytes([3; 16])),
        joint_count: JOINTS as u32,
        morph_target_count: 4,
        rate: DEFAULT_RATE,
        ticks: 60,
        ..Clip::default()
    };
    for joint in 0..JOINTS {
        clip.rotations.push(RotationTrack { joint: joint as u16, count: 2, first: 2 * joint as u32 });
        clip.rotation_ticks.extend([0, 60]);
        clip.rotation_keys
            .extend([PackedRotation::encode([0.0, 0.0, 0.0, 1.0]), PackedRotation::encode(turn)]);
    }
    clip
}

/// A rig for the chain, with its first joints as every bone a humanoid must have, and a blink.
fn rig() -> CharacterRig {
    let mut humanoid: Vec<HumanoidJoint> = HumanBone::REQUIRED
        .iter()
        .enumerate()
        .map(|(joint, &bone)| HumanoidJoint { bone, joint: joint as u16 })
        .collect();
    humanoid.sort_by_key(|entry| entry.bone);
    CharacterRig {
        joint_count: JOINTS as u32,
        morph_target_count: 4,
        humanoid,
        expressions: vec![Expression {
            name: "blink".to_owned(),
            preset: Some(ExpressionPreset::Blink),
            binary: false,
            override_blink: ExpressionOverride::None,
            override_look_at: ExpressionOverride::None,
            override_mouth: ExpressionOverride::None,
            morph_targets: vec![MorphTargetWeight { target: 1, weight: 1.0 }],
            material_colors: Vec::new(),
            texture_transforms: Vec::new(),
        }],
        ..CharacterRig::default()
    }
}

const GRAPH: &str = r#"(
    type: "sapphire::assets::AnimGraph",
    version: 1,
    data: (
        clips: [(name: "chain", source: "9a29e8c0-f131-4ea4-98b9-04879740413d")],
        masks: [(name: "torso", bones: [(bone: Spine, weight: 1.0)])],
        locomotion: (
            start: "ground",
            states: [
                (name: "ground", plays: Blend((samples: [
                    (clip: "chain", at: (0.0, 0.0), sync: false),
                    (clip: "chain", at: (0.0, 1.5), phase: 0.5),
                    (clip: "chain", at: (0.0, 5.0)),
                    (clip: "chain", at: (4.0, 2.0)),
                    (clip: "chain", at: (-4.0, 2.0)),
                    (clip: "chain", at: (0.0, -1.0)),
                ]))),
                (name: "air", plays: Clip(clip: "chain", looping: true)),
            ],
            transitions: [
                (from: "ground", to: "air", when: Airborne, decay: 8),
                (from: "air", to: "ground", when: Landed, decay: 4),
            ],
        ),
        combat: (decay: 5, exit: 12, moves: [
            (move: "jab", clip: "chain"),
            (move: "hook", clip: "chain", mask: "torso"),
        ]),
        additive: [(name: "breathing", clip: "chain", mask: "torso", weight: 0.5)],
        reactions: [(name: "flinch", clip: "chain", mask: "torso", weight: 0.5, fade_out: 8)],
        blink: Some((expression: "blink", every: (20, 40), close: 3, hold: 1, open: 7)),
    ),
)"#;

/// Frame `frame` of a run through everything: running, in the air and landing, two moves and a cancel,
/// reactions, and a cut.
fn input(frame: u32) -> GraphInput {
    let cycle = frame % 400;
    let mut input = GraphInput {
        ticks: f64::from(frame) * 0.75,
        locomotion: LocomotionInput {
            speed: 2.0 + 3.0 * (frame % 90) as f32 / 90.0,
            direction: frame as f32 * 0.01,
            grounded: !(100..130).contains(&cycle),
            landed: cycle == 130,
        },
        reaction: ReactionInput { name: Name::new("flinch"), count: frame / 170 },
        ..GraphInput::default()
    };
    if (200..240).contains(&cycle) {
        let position = f64::from(cycle - 200) * 0.75;
        input.combat = Some(CombatInput { name: Name::new("jab"), serial: 1 + frame / 400 * 3, position });
    } else if (240..280).contains(&cycle) {
        let position = f64::from(cycle - 240) * 0.75;
        input.combat = Some(CombatInput { name: Name::new("hook"), serial: 2 + frame / 400 * 3, position });
    }
    input.cut = cycle == 390;
    input
}

#[test]
fn playing_a_graph_allocates_nothing_once_it_is_made() {
    let skeleton_bytes = skeleton().to_cooked().expect("cooked");
    let cooked = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let clip_bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&clip_bytes).expect("a valid clip");
    let rig_bytes = rig().to_cooked().expect("cooked");
    let rig = CharacterRig::read(&rig_bytes).expect("a valid rig");
    let mut graph = AnimGraph::from_document(GRAPH).expect("a good document");
    let id = AssetId::from_bytes([3; 16]);
    graph.resolve(|_| id);
    let clips = [(id, clip)];
    let source = clips.as_slice();
    let binding = Arc::new(GraphBinding::new(&graph, cooked, Some(rig), 4, source).expect("it binds"));
    let replacement = Arc::new(GraphBinding::new(&graph, cooked, Some(rig), 4, source).expect("it binds"));
    let pose = CharacterPose::new(cooked, &[], &[], vec![0.0; 4]);
    let mut player = GraphPlayer::new(binding, pose, 7);

    let (evaluated, counts) = alloc::count_in_scope(|| {
        let mut evaluated = 0;
        for frame in 0..2000 {
            if frame == 700 {
                player.replace(replacement.clone());
            }
            let mut frame_input = input(frame);
            // Every so often the clock stands still, on a frame already drawn.
            if frame % 50 == 49 {
                frame_input.ticks -= 0.75;
            }
            evaluated += u32::from(player.evaluate(&frame_input, source, 0));
            player.finish();
        }
        evaluated
    });
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
    assert!(evaluated > 1000, "{evaluated}");
    assert!(player.stats().transitions >= 10, "{:?}", player.stats());
    assert!(player.teleports() >= 4, "{}", player.teleports());
    assert!(Arc::ptr_eq(player.graph(), &replacement), "the replacement took effect");
}
