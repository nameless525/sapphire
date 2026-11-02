//! What a character's frame costs on this machine, step by step, on a heavy clip: every joint turning
//! with a key every other tick. Not a gate, since it prints rather than asserts, but the number N2's
//! decision rests on (scalar is enough, spec §5.13), for whoever changes the hot loops or asks what a
//! graph with more layers would cost. Run it in release, on an idle machine if the numbers matter:
//! `cargo test --release -p sapphire_anim --test cost -- --ignored --nocapture`.

#![allow(clippy::print_stdout, reason = "this test's output is what it's for")]
#![allow(clippy::disallowed_methods, reason = "a timing test reads the OS clock, and reads nothing else")]

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use sapphire_anim::{
    AnimGraph, ArchivedClip, ArchivedSkeleton, CharacterPose, Clip, CombatInput, GraphBinding, GraphInput,
    GraphPlayer, Joint, JointMask, JointTransform, LocomotionInput, PackedRotation, RotationTrack, Skeleton,
    Transform, VectorRange, VectorTrack, add, blend, make_delta, sample,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::DEFAULT_RATE;
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::{AimAxis, Axis, Constraint, ConstraintKind};
use sapphire_core::Name;
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};

/// The look-dev VRM has about this many joints, hair and helpers among them.
const JOINTS: usize = 128;
const TICKS: u16 = 60;
const ROUNDS: u32 = 2_000;

fn skeleton() -> Skeleton {
    let joints: Vec<Joint> = (0..JOINTS)
        .map(|joint| Joint {
            name: format!("joint {joint}"),
            // A body's shape: chains of six.
            parent: (joint > 0).then(|| if joint % 6 == 0 { 0 } else { joint as u16 - 1 }),
            rest: JointTransform { translation: [0.05, 0.1, 0.0], ..JointTransform::IDENTITY },
        })
        .collect();
    Skeleton::new(&joints)
}

/// Every joint turning, with a key every other tick; the first few moving too.
fn clip() -> Clip {
    let keys = usize::from(TICKS) / 2 + 1;
    let mut clip = Clip {
        name: "heavy".into(),
        skeleton: Some(AssetId::from_bytes([4; 16])),
        joint_count: JOINTS as u32,
        rate: DEFAULT_RATE,
        ticks: TICKS,
        ..Clip::default()
    };
    for joint in 0..JOINTS {
        let first = clip.rotation_keys.len() as u32;
        clip.rotations.push(RotationTrack { joint: joint as u16, count: keys as u16, first });
        for key in 0..keys {
            let angle = (joint + key) as f32 * 0.03;
            clip.rotation_ticks.push(key as u16 * 2);
            clip.rotation_keys.push(PackedRotation::encode(Quat::from_rotation_y(angle).to_array()));
        }
    }
    let range = VectorRange::enclosing([[0.0; 3], [0.1, 0.2, 0.1]]);
    for joint in 0..4 {
        let first = clip.vector_keys.len() as u32;
        clip.translations.push(VectorTrack { joint, count: keys as u16, first, range });
        for key in 0..keys {
            clip.vector_ticks.push(key as u16 * 2);
            clip.vector_keys.push(range.encode([key as f32 * 0.001, 0.1, 0.0]));
        }
    }
    clip
}

/// The graph's own frames: returns what one takes, at a walk-to-run blend with breathing.
fn graph_costs(skeleton: &ArchivedSkeleton, clip: &ArchivedClip) -> f64 {
    let id = AssetId::from_bytes([4; 16]);
    let text = r#"(
        type: "sapphire::assets::AnimGraph",
        version: 1,
        data: (
            clips: [(name: "c", source: "9a29e8c0-f131-4ea4-98b9-04879740413d")],
            locomotion: (start: "ground", smoothing: 0.0, states: [(name: "ground", plays: Blend((samples: [
                (clip: "c", at: (0.0, 0.0), sync: false),
                (clip: "c", at: (0.0, 2.0)),
                (clip: "c", at: (0.0, 6.0)),
            ])))]),
            combat: (decay: 5, exit: 12, moves: [(move: "a", clip: "c"), (move: "b", clip: "c")]),
            additive: [(name: "breathing", clip: "c", weight: 0.5)],
        ),
    )"#;
    let mut graph = AnimGraph::from_document(text).expect("a good document");
    graph.resolve(|_| id);
    let clips = [(id, clip)];
    let source = clips.as_slice();
    let binding = Arc::new(GraphBinding::new(&graph, skeleton, None, 8, source).expect("it binds"));
    let mut player = GraphPlayer::new(binding, CharacterPose::new(skeleton, &[], &[], vec![0.0; 8]), 1);
    let mut ticks = 0.0;
    let walking = LocomotionInput { speed: 4.0, ..LocomotionInput::default() };
    let frame = time("graph: a blend of two with an additive", |_| {
        ticks += 1.0;
        let input = GraphInput { ticks, locomotion: walking, ..GraphInput::default() };
        player.evaluate(&input, source, 0);
        player.finish();
        black_box(player.pose());
    });
    time("graph: a frozen clock's frame", |_| {
        let input = GraphInput { ticks, locomotion: walking, ..GraphInput::default() };
        black_box(player.evaluate(&input, source, 0));
    });
    let mut serial = 0;
    time("graph: a cancel's frame, every frame", |_| {
        ticks += 1.0;
        serial += 1;
        let combat = CombatInput { name: Name::new("a"), serial, position: 0.0 };
        let input = GraphInput { ticks, combat: Some(combat), ..GraphInput::default() };
        player.evaluate(&input, source, 0);
        player.finish();
        black_box(player.pose());
    });
    // Every round inside the last cancel's five ticks.
    time("graph: a frame under a decay", |_| {
        ticks += 4.0 / f64::from(ROUNDS);
        let combat = CombatInput { name: Name::new("a"), serial, position: 2.0 };
        let input = GraphInput { ticks, combat: Some(combat), ..GraphInput::default() };
        player.evaluate(&input, source, 0);
        player.finish();
        black_box(player.pose());
    });
    frame
}

/// `step`'s mean in µs over [`ROUNDS`] runs, at positions across the clip.
fn time(name: &str, mut step: impl FnMut(f32)) -> f64 {
    let started = Instant::now();
    for round in 0..ROUNDS {
        step((round % u32::from(TICKS)) as f32 + 0.5);
    }
    let micros = started.elapsed().as_secs_f64() * 1e6 / f64::from(ROUNDS);
    println!("{name:44} {micros:8.2} µs");
    micros
}

#[test]
#[ignore = "prints what a frame costs; run it in release"]
fn a_character_frame_step_by_step() {
    let skeleton_bytes = skeleton().to_cooked().expect("cooked");
    let cooked = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
    let clip_bytes = clip().to_cooked().expect("cooked");
    let clip = Clip::read(&clip_bytes).expect("a valid clip");
    let binds: Vec<SkinBind> = (0..JOINTS)
        .map(|joint| SkinBind {
            joint: joint as u16,
            inverse_bind_matrix: Mat4::from_translation(Vec3::NEG_Y).to_cols_array_2d(),
        })
        .collect();
    // As the look-dev VRM has them: rolls down the arms and legs, a few aims.
    let constraints: Vec<Constraint> = (0..24)
        .map(|index| Constraint {
            joint: 5 + 6 * index,
            source: 4 + 6 * index,
            weight: 0.5,
            kind: if index % 4 == 0 {
                ConstraintKind::Aim { axis: AimAxis::PositiveX }
            } else {
                ConstraintKind::Roll { axis: Axis::X }
            },
        })
        .collect();
    let mut pose = CharacterPose::new(cooked, &binds, &constraints, vec![0.0; 8]);
    let rest = pose.rest().to_vec();
    let mut other = rest.clone();
    let mut delta = vec![Transform::IDENTITY; JOINTS];
    let mut palette = vec![Affine3A::IDENTITY; pose.palette_len()];
    let mut mask = JointMask::nothing(JOINTS);
    mask.set_subtree(cooked, 1, 1.0);

    println!(
        "{JOINTS} joints, {} constraints, a rotation key every 2 ticks on every joint:",
        constraints.len()
    );
    let sampled = time("sample the clip", |position| {
        sample(clip, position, &rest, &mut other);
        black_box(&other);
    });
    time("blend two poses", |position| {
        blend(pose.locals_mut(), &other, position / 120.0, None);
        black_box(pose.locals());
    });
    time("blend two poses through a mask", |position| {
        blend(pose.locals_mut(), &other, position / 120.0, Some(&mask));
        black_box(pose.locals());
    });
    time("make an additive delta", |_| {
        make_delta(&other, &rest, &mut delta);
        black_box(&delta);
    });
    time("add an additive layer", |position| {
        add(pose.locals_mut(), &delta, position / 120.0, None);
        black_box(pose.locals());
    });
    let finish = time("constraints and local-to-model", |_| {
        pose.finish();
        black_box(pose.model());
    });
    let write = time("the palette", |_| {
        pose.write_palette(&mut palette);
        black_box(&palette);
    });
    let frame = time("a frame: one clip to the palette", |position| {
        pose.sample(clip, position);
        pose.finish();
        pose.write_palette(&mut palette);
        black_box(&palette);
    });
    let layered = time("a heavy graph's frame: 4 clips, 2 blends, 1 additive", |position| {
        pose.sample(clip, position);
        sample(clip, position + 7.0, &rest, &mut other);
        blend(pose.locals_mut(), &other, 0.5, None);
        sample(clip, position + 13.0, &rest, &mut other);
        blend(pose.locals_mut(), &other, 0.5, Some(&mask));
        sample(clip, position + 29.0, &rest, &mut other);
        make_delta(&other, &rest, &mut delta);
        add(pose.locals_mut(), &delta, 0.25, None);
        pose.finish();
        pose.write_palette(&mut palette);
        black_box(&palette);
    });
    let graph_frame = graph_costs(cooked, clip);
    println!(
        "a graph's frame at a walk-to-run blend with breathing, by the player, {graph_frame:.1} µs, with every joint \
         animated, which real clips don't"
    );
    println!(
        "against one clip's sample {sampled:.1}, finish {finish:.1}, palette {write:.1}: a frame {frame:.1} µs, a heavy \
         graph's {layered:.1} µs; budgets 1,500 µs on Desktop-Low and 3,000 µs on Mobile-Low"
    );
}
