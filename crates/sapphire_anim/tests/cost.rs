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
    AnimGraph, ArchivedClip, ArchivedSkeleton, CharacterPose, Clip, ClothSheet, CombatInput, FeetInput,
    FootGround, GraphBinding, GraphInput, GraphPlayer, Grip, GroundHit, Hand, HeldWeapon, IkInput, Joint,
    JointMask, JointTransform, LocomotionInput, LookInput, PackedRotation, RotationTrack, Skeleton,
    SpringBones, SpringFrame, Substeps, Transform, VectorRange, VectorTrack, WeaponPlace, add, blend,
    make_delta, sample,
};
use sapphire_assets::AssetId;
use sapphire_assets::clip::DEFAULT_RATE;
use sapphire_assets::mesh::SkinBind;
use sapphire_assets::rig::{
    AimAxis, Axis, CharacterRig, Collider as RigCollider, ColliderGroup, ColliderShape, Constraint,
    ConstraintKind, Spring, SpringJoint, SpringTip,
};
use sapphire_core::Name;
use sapphire_math::{Affine3A, Mat4, Quat, Vec3};

mod humanoid;

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
        "{JOINTS} joints, {} active constraints ({} authored), a rotation key every 2 ticks on every joint:",
        constraints
            .iter()
            .filter(|c| usize::from(c.joint) < JOINTS && usize::from(c.source) < JOINTS)
            .count(),
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

/// IK's stages on a humanoid of the same size, every stage working: feet on a slope, a moving look
/// target, a two-handed weapon. Ground casts are the caller's, so `sapphire_physics` times them.
#[test]
#[ignore = "prints what IK costs; run it in release"]
fn ik_stage_by_stage() {
    let (skeleton, bones) = humanoid::humanoid(JOINTS - 21);
    let bytes = skeleton.to_cooked().expect("cooked");
    let cooked = Skeleton::read(&bytes).expect("a valid skeleton");
    let mut pose = CharacterPose::new(cooked, &[], &[], Vec::new());
    let mut ik = humanoid::ik(&pose, &bones);
    let world = Affine3A::from_translation(Vec3::new(1.0, 0.04, -1.0));
    let normal = Vec3::new(-0.5, 0.866, 0.0);
    let hit = |at: Vec3| {
        let height = 0.04 + (at.x - 1.0) * 0.5 / 0.866;
        Some(GroundHit { point: Vec3::new(at.x, height, at.z), normal })
    };
    let weapon = HeldWeapon {
        place: WeaponPlace::Joint { joint: 19, offset: Affine3A::from_translation(Vec3::X * 0.1) },
        main: Hand::Right,
        main_grip: None,
        off_grip: Some(Grip {
            on_weapon: Transform::from_translation(Vec3::new(-0.3, 0.0, 0.1)),
            in_hand: Transform::from_translation(Vec3::X * 0.05),
            pole: None,
            weight: 1.0,
        }),
    };
    let rest = pose.rest().to_vec();
    let ground = |pose: &mut CharacterPose, ik: &sapphire_anim::CharacterIk| {
        let probes = ik.foot_probes(pose, &world).expect("it has feet");
        let foot = |side: usize| FootGround {
            ankle: hit(probes[side].ankle),
            ball: probes[side].ball.and_then(hit),
        };
        [foot(0), foot(1)]
    };
    let none = IkInput {
        dt: 1.0 / 60.0,
        world_from_model: world,
        feet: FeetInput::default(),
        look: LookInput::default(),
        weapon: None,
    };
    let target = |position: f32| Vec3::new(4.0 * position.sin(), 1.5, 4.0 * position.cos());

    println!("{JOINTS} joints, as a character's, IK reading the 21 of them that are its body:");
    let plain = time("the pose alone: reset and finish", |_| {
        pose.locals_mut().copy_from_slice(&rest);
        pose.finish();
        black_box(pose.model());
    });
    let prepare = time("what IK reads, up to date", |_| {
        pose.locals_mut().copy_from_slice(&rest);
        ik.prepare(&mut pose);
        black_box(pose.model());
    });
    let feet = time("feet on a slope", |_| {
        pose.locals_mut().copy_from_slice(&rest);
        ik.prepare(&mut pose);
        let ground = ground(&mut pose, &ik);
        ik.solve(&mut pose, &IkInput { feet: FeetInput { weight: 1.0, ground }, ..none });
        black_box(pose.locals());
    });
    ik.reset();
    let look = time("a look at a moving target", |position| {
        pose.locals_mut().copy_from_slice(&rest);
        ik.prepare(&mut pose);
        let look = LookInput { target: Some(target(position * 0.1)), weight: 1.0 };
        ik.solve(&mut pose, &IkInput { look, ..none });
        black_box(pose.locals());
    });
    ik.reset();
    let hands = time("the off hand on a carried weapon", |_| {
        pose.locals_mut().copy_from_slice(&rest);
        ik.prepare(&mut pose);
        ik.solve(&mut pose, &IkInput { weapon: Some(weapon), ..none });
        black_box(pose.locals());
    });
    ik.reset();
    let all = time("all of it, then finish", |position| {
        pose.locals_mut().copy_from_slice(&rest);
        ik.prepare(&mut pose);
        let ground = ground(&mut pose, &ik);
        ik.solve(
            &mut pose,
            &IkInput {
                feet: FeetInput { weight: 1.0, ground },
                look: LookInput { target: Some(target(position * 0.1)), weight: 1.0 },
                weapon: Some(weapon),
                ..none
            },
        );
        pose.finish();
        black_box(pose.model());
    });
    println!(
        "reads {prepare:.2}; feet {:.2}, look {:.2}, hands {:.2} over them; whole {:.2} µs over the plain pose's {plain:.2}",
        feet - prepare,
        look - prepare,
        hands - prepare,
        all - plain,
    );
}

/// What chain cloth costs a sub-step, on the sheet M5 C3 reports: a skirt of eight chains of eight
/// swinging joints round the hips as one closed sheet, with two leg capsules, on a walk at 60 frames a
/// second and two sub-steps a tick. The same chains as springs alone (0 iterations) beside 4, 2 and 1, in
/// interleaved rounds, the least round of each.
#[test]
#[ignore = "prints what cloth costs; run it in release"]
fn a_cloth_sheet_by_iterations() {
    const CHAINS: usize = 8;
    const DEPTH: usize = 8;
    const FRAMES: u32 = 1_200;
    const LAPS: u32 = 7;
    let joint = |name: String, parent: usize, translation: [f32; 3]| Joint {
        name,
        parent: Some(parent as u16),
        rest: JointTransform { translation, ..JointTransform::IDENTITY },
    };
    let mut joints = vec![
        Joint { name: "root".into(), parent: None, rest: JointTransform::IDENTITY },
        joint("hips".into(), 0, [0.0, 1.0, 0.0]),
        joint("left leg".into(), 1, [0.09, 0.0, 0.0]),
        joint("right leg".into(), 1, [-0.09, 0.0, 0.0]),
    ];
    for chain in 0..CHAINS {
        let (sin, cos) = (std::f32::consts::TAU * chain as f32 / CHAINS as f32).sin_cos();
        joints.push(joint(format!("skirt {chain} 0"), 1, [0.18 * cos, -0.05, 0.18 * sin]));
        for at in 1..DEPTH {
            let parent = joints.len() - 1;
            joints.push(joint(format!("skirt {chain} {at}"), parent, [0.015 * cos, -0.06, 0.015 * sin]));
        }
    }
    let leg = |joint: u16| RigCollider {
        joint,
        shape: ColliderShape::Capsule { offset: [0.0; 3], radius: 0.07, tail: [0.0, -0.8, 0.0] },
    };
    let rig = CharacterRig {
        joint_count: joints.len() as u32,
        springs: (0..CHAINS)
            .map(|chain| Spring {
                name: format!("skirt {chain}"),
                joints: (0..DEPTH)
                    .map(|at| SpringJoint {
                        joint: (4 + chain * DEPTH + at) as u16,
                        hit_radius: 0.02,
                        stiffness: 0.3,
                        gravity_power: 0.5,
                        gravity_dir: [0.0, -1.0, 0.0],
                        drag_force: 0.2,
                    })
                    .collect(),
                tip: SpringTip::Extended { length: 0.06 },
                collider_groups: vec![0],
                center: None,
            })
            .collect(),
        colliders: vec![leg(2), leg(3)],
        collider_groups: vec![ColliderGroup { name: "legs".into(), colliders: vec![0, 1] }],
        ..CharacterRig::default()
    };
    let rig_bytes = rig.to_cooked().expect("cooked");
    let rig = CharacterRig::read(&rig_bytes).expect("a valid rig");
    let skeleton_bytes = Skeleton::new(&joints).to_cooked().expect("cooked");
    let sheet = ClothSheet {
        chains: (0..CHAINS).map(|chain| format!("skirt {chain}")).collect(),
        closed: true,
        lateral_compliance: 1e-6,
        bend_compliance: 1e-4,
    };

    let counts = [0, 4, 2, 1];
    let mut least = [f64::MAX; 4];
    let mut substeps = 0;
    for _ in 0..LAPS {
        for (index, &iterations) in counts.iter().enumerate() {
            let skeleton = Skeleton::read(&skeleton_bytes).expect("a valid skeleton");
            let mut pose = CharacterPose::new(skeleton, &[], &[], Vec::new());
            let mut springs = SpringBones::new(rig, &pose).expect("it has springs");
            springs.set_substeps(Substeps::Two);
            springs.set_cloth(rig, std::slice::from_ref(&sheet)).expect("its chains");
            springs.set_cloth_iterations(iterations);
            let mut spent = 0.0;
            substeps = 0;
            for frame in 0..FRAMES {
                let seconds = frame as f32 / 60.0;
                let wave = (seconds * std::f32::consts::TAU).sin();
                pose.reset();
                let locals = pose.locals_mut();
                locals[1].rotation = Quat::from_rotation_y(wave * 0.2);
                locals[2].rotation = Quat::from_rotation_x(-wave * 0.5);
                locals[3].rotation = Quat::from_rotation_x(wave * 0.5);
                pose.finish();
                let frame = SpringFrame {
                    world_from_model: Affine3A::from_translation(Vec3::new(0.0, 0.0, seconds * 1.4)),
                    ticks: f64::from(frame),
                    pose_moved: true,
                    active: true,
                };
                let started = Instant::now();
                let work = springs.update(&mut pose, &frame);
                spent += started.elapsed().as_secs_f64();
                substeps += work.substeps;
                black_box(pose.model());
            }
            least[index] = least[index].min(spent * 1e6 / f64::from(substeps));
        }
    }
    println!(
        "{CHAINS} chains of {DEPTH} swinging joints as one closed sheet, two capsules, {substeps} sub-steps a round:"
    );
    for (iterations, micros) in counts.iter().zip(least) {
        println!(
            "{iterations} iterations: {micros:6.2} µs a sub-step, {:6.2} of it the cloth's",
            micros - least[0]
        );
    }
}
