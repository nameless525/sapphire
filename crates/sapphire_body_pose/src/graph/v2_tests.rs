//! Graph v2's runtime against v1's: every shipped graph, converted and compiled, binds and plays v1's
//! poses bit for bit, frame for frame, on both maths, through a scripted session that reaches each of
//! its states, transitions and rules and every reset of the smoothed velocity (M6 A5b).
//!
//! The clips are synthetic, keyed by each graph's own clip IDs on a humanoid skeleton with every bone,
//! so no cooked content is needed. The graphs are read from the content tree, never named: a boss's
//! graph is content (gameplay's `bosses_are_content`).

use std::sync::Arc;

use sapphire_assets::AssetId;
use sapphire_core::Name;
use sapphire_time::{SUBTICKS_PER_TICK, SimTime};

use super::canonical::Canonical;
use super::*;

use crate as body;
#[path = "../../tests/support/humanoid.rs"]
mod humanoid;
use humanoid::{Humanoid, Step, session, shipped, version_1};
use sapphire_assets::clip::{PackedRotation, RotationTrack};
use sapphire_assets::rig::HumanBone;
use sapphire_math::Vec3;

fn bits(values: &[f32]) -> Vec<u32> {
    values.iter().map(|value| value.to_bits()).collect()
}

fn transform_bits(pose: &[Transform]) -> Vec<u32> {
    pose.iter()
        .flat_map(|joint| {
            joint
                .translation
                .to_array()
                .into_iter()
                .chain(joint.rotation.to_array())
                .chain(joint.scale.to_array())
        })
        .map(f32::to_bits)
        .collect()
}

/// Everything a player's history holds, as the fixed step's checksum lays it out.
fn canonical(player: &GraphPlayer) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut sink = |chunk: &[u8]| bytes.extend_from_slice(chunk);
    Canonical(&mut sink).player(player);
    bytes
}

/// Plays `steps` on two players and holds them bit for bit each frame. Returns each frame's samples on
/// the second player, and what it reached.
fn presentation_matches(
    name: &str,
    humanoid: &Humanoid,
    old: &Arc<GraphBinding>,
    new: &Arc<GraphBinding>,
    steps: &[Step],
) -> (Vec<u64>, u64) {
    let source = humanoid.source();
    let clips = source.as_slice();
    let mut players = [old, new].map(|graph| GraphPlayer::new(Arc::clone(graph), humanoid.pose(), 11));
    let mut samples = Vec::with_capacity(steps.len());
    for (frame, step) in steps.iter().enumerate() {
        let mut changed = [false; 2];
        let mut counts = [0; 2];
        for (index, player) in players.iter_mut().enumerate() {
            if step.replace {
                player.replace(Arc::clone(if index == 0 { old } else { new }));
            }
            if let Some(released) = step.gather {
                player.request_grounded_gather(released);
            }
            let before = sample::SAMPLED.with(std::cell::Cell::get);
            changed[index] = player.evaluate_with_support(&step.input, clips, 0, step.turn, step.ground);
            counts[index] = sample::SAMPLED.with(std::cell::Cell::get) - before;
        }
        let [v1, v2] = &players;
        let at = || format!("{name}, frame {frame} at tick {}", step.input.ticks);
        assert_eq!(changed[0], changed[1], "{}: whether it changed", at());
        assert_eq!(counts[0], counts[1], "{}: clips sampled", at());
        assert_eq!(v1.state(), v2.state(), "{}: state", at());
        assert_eq!(v1.stats(), v2.stats(), "{}: stats", at());
        assert_eq!(v1.teleports(), v2.teleports(), "{}: teleports", at());
        assert_eq!(transform_bits(v1.pose().locals()), transform_bits(v2.pose().locals()), "{}: pose", at());
        assert_eq!(bits(v1.pose().weights()), bits(v2.pose().weights()), "{}: weights", at());
        assert_eq!(transform_bits(v1.target()), transform_bits(v2.target()), "{}: target", at());
        assert_eq!(
            format!("{:?}", v1.ground_foot_contact()),
            format!("{:?}", v2.ground_foot_contact()),
            "{}: foot contact",
            at()
        );
        assert_eq!(
            v1.grounded_gather().map(f32::to_bits),
            v2.grounded_gather().map(f32::to_bits),
            "{}",
            at()
        );
        assert_eq!(
            format!("{:?}", v1.landing_root(clips)),
            format!("{:?}", v2.landing_root(clips)),
            "{}: landing root",
            at()
        );
        assert_eq!(canonical(v1), canonical(v2), "{}: history", at());
        samples.push(counts[1]);
    }
    let [v1, v2] = &players;
    assert_eq!(v1.reached, v2.reached, "{name}: what the script reached");
    (samples, v2.reached)
}

/// Plays `steps` through two fixed-step histories and holds their checksum bytes bit for bit.
fn simulation_matches(
    name: &str,
    humanoid: &Humanoid,
    old: &Arc<GraphBinding>,
    new: &Arc<GraphBinding>,
    steps: &[Step],
) {
    let source = humanoid.source();
    let mut histories = [old, new].map(|graph| {
        FixedGraphHistory::try_new(Arc::clone(graph), humanoid.pose(), 11, source.as_slice())
            .unwrap_or_else(|error| panic!("{name}: {error}"))
    });
    let mut generation = 0;
    for (frame, step) in steps.iter().enumerate() {
        if step.input.cut {
            generation += 1;
        }
        let stamp = GraphStepStamp {
            step: frame as u64,
            owner_now: SimTime::from_subticks((step.input.ticks * f64::from(SUBTICKS_PER_TICK)) as u64),
            generation,
        };
        let mut changed = [false; 2];
        for (index, history) in histories.iter_mut().enumerate() {
            let prepared = history.prepare(stamp, step.input, false).expect("a step forward");
            changed[index] =
                history.advance_prepared(prepared, step.turn, step.ground, step.gather.unwrap_or(false));
        }
        let at = || format!("{name}, fixed step {frame} at tick {}", step.input.ticks);
        assert_eq!(changed[0], changed[1], "{}: whether it changed", at());
        let checksum = |history: &FixedGraphHistory<&[(AssetId, &ArchivedClip)]>| {
            let mut bytes = Vec::new();
            history.write_canonical(&mut |chunk: &[u8]| bytes.extend_from_slice(chunk));
            bytes
        };
        assert_eq!(checksum(&histories[0]), checksum(&histories[1]), "{}: checksum", at());
    }
}

/// The heroine's graph is the one with a machine of states: the script must reach each transition
/// and each reset of the smoothed velocity, or the match proves less than it says.
fn must_reach(name: &str, binding: &GraphBinding, reached: u64) {
    if binding.changes.is_empty() {
        return;
    }
    let all = (1u64 << binding.changes.len()) - 1;
    let missing: Vec<usize> =
        (0..binding.changes.len()).filter(|&index| reached & (1 << index) == 0).collect();
    assert_eq!(reached & all, all, "{name}: the script never takes transitions {missing:?}");
    let resets = [
        ("gather", Reached::GATHER),
        ("frozen clock", Reached::FROZEN),
        ("landing hand-off", Reached::HANDOFF),
        ("turn", Reached::TURN),
        ("start over", Reached::START_OVER),
        ("launch catch", Reached::LAUNCH),
    ];
    for (reset, bit) in resets {
        assert!(reached & bit != 0, "{name}: the script never resets the smoothed velocity by a {reset}");
    }
}

/// FNV-1a over 64 bits: the same digest on every machine and every toolchain.
struct Digest(u64);

impl Digest {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// What `binding` plays through `steps`, as two digests that read the same on every machine: the fixed
/// step's checksum bytes each step (pose, weights, target and the whole history, on the deterministic
/// maths), and presentation's clips sampled, state, stats, teleports and foot contact each frame and
/// what it reached.
fn played(humanoid: &Humanoid, binding: &Arc<GraphBinding>, steps: &[Step]) -> (u64, u64) {
    let source = humanoid.source();
    let clips = source.as_slice();
    let mut history = FixedGraphHistory::try_new(Arc::clone(binding), humanoid.pose(), 11, clips)
        .unwrap_or_else(|error| panic!("{error}"));
    let mut player = GraphPlayer::new(Arc::clone(binding), humanoid.pose(), 11);
    let (mut fixed, mut shown) = (Digest::new(), Digest::new());
    let mut generation = 0;
    for (frame, step) in steps.iter().enumerate() {
        if step.input.cut {
            generation += 1;
        }
        let stamp = GraphStepStamp {
            step: frame as u64,
            owner_now: SimTime::from_subticks((step.input.ticks * f64::from(SUBTICKS_PER_TICK)) as u64),
            generation,
        };
        let prepared = history.prepare(stamp, step.input, false).expect("a step forward");
        let changed =
            history.advance_prepared(prepared, step.turn, step.ground, step.gather.unwrap_or(false));
        fixed.write(&[u8::from(changed)]);
        history.write_canonical(&mut |chunk: &[u8]| fixed.write(chunk));

        if step.replace {
            player.replace(Arc::clone(binding));
        }
        if let Some(released) = step.gather {
            player.request_grounded_gather(released);
        }
        let before = sample::SAMPLED.with(std::cell::Cell::get);
        let changed = player.evaluate_with_support(&step.input, clips, 0, step.turn, step.ground);
        let sampled = sample::SAMPLED.with(std::cell::Cell::get) - before;
        shown.write(&[u8::from(changed)]);
        shown.write(&sampled.to_le_bytes());
        shown.write(player.state().to_string().as_bytes());
        let stats = player.stats();
        for count in [stats.transitions, stats.unmapped_moves, stats.missing_clips, player.teleports()] {
            shown.write(&count.to_le_bytes());
        }
        shown.write(format!("{:?}", player.ground_foot_contact()).as_bytes());
    }
    shown.write(&player.reached.to_le_bytes());
    (fixed.0, shown.0)
}

/// What each shipped graph plays through the session, in [`shipped`]'s order (by path; a boss's graph is
/// content, so no path is named here): [`played`]'s two digests, taken from version 1's player before
/// the switch (M6 A5c). A new shipped graph adds its row.
const PLAYED: [(u64, u64); 7] = [
    (0x6672_79b6_4c92_18b5, 0xc621_2f18_d6c2_afc6),
    (0x5ef1_844f_2257_309e, 0x1287_50ea_9c42_cfeb),
    (0x677d_a669_6679_be33, 0x60b0_7dde_1999_5b2e),
    (0x41d3_096e_1ec1_0102, 0x635a_f86c_e5f1_1c7f),
    (0xe154_eb74_2d99_313d, 0x8a25_ea98_e032_317a),
    (0x4493_f9c4_5842_eab6, 0x9241_2dae_1049_254a),
    (0xb401_d98f_b0c1_6085, 0xe2ae_71cc_0f1a_1f9f),
];

/// A5c's bar, kept once v1 is gone: each shipped graph, read, compiled for the fixed step and bound as
/// the game binds it, plays what version 1's player played, through a session that reaches every state,
/// transition, rule and reset of the smoothed velocity.
#[test]
fn every_shipped_graph_plays_what_version_1_played() {
    let shipped = shipped();
    assert_eq!(shipped.len(), PLAYED.len(), "each shipped graph has its row");
    let mut found = Vec::new();
    for (name, text) in &shipped {
        let graph = version_1(text);
        let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
        let converted = v2::convert::convert(&graph);
        let program =
            converted.compile_for(Target::Simulation).unwrap_or_else(|error| panic!("{name}: {error}"));
        let binding = humanoid.bind(&program);
        let steps = session(&converted);
        let old = played(&humanoid, &humanoid.bind_v1(&graph), &steps);
        assert_eq!(played(&humanoid, &binding, &steps), old, "{name}: the program against v1's player");
        found.push(old);
    }
    assert_eq!(found, PLAYED, "{found:#x?}");
}

/// A5c's bar: v2 evaluating each shipped graph's conversion gives v1's poses bit for bit, frame for
/// frame, on presentation's maths and the fixed step's, through a session that reaches every state,
/// transition, rule and reset of the smoothed velocity.
#[test]
fn every_shipped_graph_plays_v1s_poses_bit_for_bit_on_both_maths() {
    for (name, text) in shipped() {
        let graph = version_1(&text);
        let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
        let old = humanoid.bind_v1(&graph);
        let converted = v2::convert::convert(&graph);
        let presentation =
            humanoid.bind(&converted.compile().unwrap_or_else(|error| panic!("{name}: {error}")));
        let simulation = humanoid.bind(
            &converted.compile_for(Target::Simulation).unwrap_or_else(|error| panic!("{name}: {error}")),
        );
        // The two front ends: v1's conditions by their variants, the program's postfix ops.
        assert!(old.changes.iter().all(|change| matches!(change.guard, Guard::Condition(_))));
        assert!(presentation.changes.iter().all(|change| matches!(change.guard, Guard::Program { .. })));
        assert_eq!(presentation.target(), Some(Target::Presentation));
        let steps = session(&v2::convert::convert(&graph));
        let (_, reached) = presentation_matches(&name, &humanoid, &old, &presentation, &steps);
        must_reach(&name, &presentation, reached);
        simulation_matches(&name, &humanoid, &old, &simulation, &steps);
    }
}

/// A program compiled for presentation may hold what only presentation runs, so the fixed step
/// refuses it; the same graph compiled for `Target::Simulation` runs there.
#[test]
fn the_fixed_step_refuses_a_program_compiled_for_presentation() {
    let (_, text) =
        shipped().into_iter().find(|(name, _)| name.contains("character")).expect("the heroine ships");
    let graph = version_1(&text);
    let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
    let converted = v2::convert::convert(&graph);
    let source = humanoid.source();
    let presentation = humanoid.bind(&converted.compile().expect("compiles"));
    assert_eq!(presentation.target(), Some(Target::Presentation));
    let refused = FixedGraphHistory::try_new(presentation, humanoid.pose(), 0, source.as_slice());
    assert!(matches!(refused, Err(GraphBindError::PresentationProgram)));
    let simulation = humanoid.bind(&converted.compile_for(Target::Simulation).expect("compiles"));
    assert_eq!(simulation.target(), Some(Target::Simulation));
    assert!(FixedGraphHistory::try_new(simulation, humanoid.pose(), 0, source.as_slice()).is_ok());
    assert_eq!(humanoid.bind_v1(&graph).target(), None, "a version 1 graph runs anywhere it did");
}

/// What each shipped graph costs a frame through the session, v2's runtime against v1's: clips
/// sampled (the mean and the worst frame), and the program's own worst case an evaluation.
#[test]
#[allow(clippy::print_stdout, reason = "the cost table is what it's for")]
fn each_shipped_graph_costs_what_v1_does() {
    let mut table = Vec::new();
    for (name, text) in shipped() {
        let graph = version_1(&text);
        let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
        let old = humanoid.bind_v1(&graph);
        let program = v2::convert::convert(&graph).compile().expect("compiles");
        let new = humanoid.bind(&program);
        let steps = session(&v2::convert::convert(&graph));
        let (samples, _) = presentation_matches(&name, &humanoid, &old, &new, &steps);
        let worst = samples.iter().copied().max().unwrap_or(0);
        let mean = samples.iter().sum::<u64>() as f64 / samples.len() as f64;
        let cost = program.cost;
        table.push(format!(
            "{name}: {} frames, clips sampled a frame {mean:.2} mean and {worst} worst (v1 the same, frame for \
             frame); an evaluation's worst case {} clips, {} registers, {} ops, {} condition ops",
            samples.len(),
            cost.clips_sampled,
            cost.registers,
            cost.ops,
            cost.condition_ops,
        ));
    }
    println!("{}", table.join("\n"));
}

/// A clip's source ID, by its place in [`FEATURE_CLIPS`].
fn source(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012}")
}

const FEATURE_CLIPS: [&str; 7] = ["idle", "walk", "run", "rise", "fall", "aim_low", "aim_high"];

/// What only version 2 says: a nested machine, a `*` transition, a 1D blend on a game slot laid over the
/// upper body by a second machine, a sync group by the left foot's landing, and events. `upper` is the
/// upper layer, if any.
fn features(upper: &str) -> String {
    let clips: Vec<String> = FEATURE_CLIPS
        .iter()
        .enumerate()
        .map(|(index, name)| format!(r#"(name: "{name}", source: "{}")"#, source(index)))
        .collect();
    format!(
        r#"(type: "sapphire::assets::AnimGraph", version: 2, data: (
        parameters: [
            (name: "velocity_x", from: Locomotion(SmoothedX)),
            (name: "velocity_z", from: Locomotion(SmoothedZ)),
            (name: "grounded", from: Locomotion(Grounded)),
            (name: "aim", from: Game("aim")),
        ],
        smoothing: 0.0,
        clips: [{}],
        masks: [(name: "upper", bones: [(bone: Spine, weight: 1.0)])],
        sync: [(name: "feet", foot: Left, contact: Down)],
        machines: [
            (name: "locomotion", start: "ground", states: [
                (name: "ground", plays: Blend2D((x: "velocity_x", y: "velocity_z", sync: Some("feet"), samples: [
                    (clip: "idle", at: (0.0, 0.0), sync: false),
                    (clip: "walk", at: (0.0, 1.5)),
                    (clip: "run", at: (0.0, 5.0)),
                ], rate: (0.5, 2.0)))),
                (name: "air", plays: Machine("air")),
            ], transitions: [
                (from: "ground", to: "air", when: "!grounded", decay: 4),
                (from: "*", to: "ground", when: "grounded", decay: 4),
            ]),
            (name: "air", start: "rise", states: [
                (name: "rise", plays: Clip(clip: "rise", looping: false)),
                (name: "fall", plays: Clip(clip: "fall", looping: true)),
            ], transitions: [
                (from: "rise", to: "fall", when: "finished", decay: 2),
            ]),
            (name: "upper", start: "aim", states: [
                (name: "aim", plays: Blend1D((parameter: "aim", samples: [(clip: "aim_low", at: -1.0), (clip: "aim_high", at: 1.0)]))),
            ]),
        ],
        layers: [
            (name: "locomotion", plays: Machine((machine: "locomotion"))),
            {upper}
        ],
        events: [
            (name: "footstep", from: Foot((foot: Left, contact: Down))),
            (name: "swish", from: Clip("swish"), min_weight: 0.5),
        ],
    ))"#,
        clips.join(", ")
    )
}

const OVER: &str =
    r#"(name: "upper", plays: Machine((machine: "upper", mask: Some("upper"), weight: 1.0))),"#;
const ADDED: &str = r#"(name: "upper", plays: Machine((machine: "upper", mask: Some("upper"), blend: Additive, weight: 1.0))),"#;

const WALK_TICKS: u16 = 30;
const LEFT_DOWN: u16 = 5;

/// The feature graph's humanoid: synthetic clips, a walk whose left foot lands on tick 5, and a run
/// with a `swish` on tick 10.
fn feature_humanoid() -> Humanoid {
    use sapphire_assets::clip::{ClipEvent, ContactTrack, FootMarker, Marker};
    let ids: Vec<AssetId> =
        (0..FEATURE_CLIPS.len()).map(|index| AssetId::from_text(&source(index))).collect();
    let mut humanoid = Humanoid::new(ids.iter().copied());
    let foot =
        |bone: HumanBone| HumanBone::ALL.iter().position(|&found| found == bone).expect("a bone") as u16;
    let marker = |tick: u16, marker: Marker| FootMarker { tick, marker };
    let contacts = vec![
        ContactTrack {
            foot: HumanBone::LeftFoot,
            joint: foot(HumanBone::LeftFoot),
            planted: false,
            markers: vec![marker(LEFT_DOWN, Marker::Down), marker(20, Marker::Up)],
            slide: 0.0,
        },
        ContactTrack {
            foot: HumanBone::RightFoot,
            joint: foot(HumanBone::RightFoot),
            planted: true,
            markers: vec![marker(3, Marker::Up), marker(18, Marker::Down)],
            slide: 0.0,
        },
    ];
    let mut walk = humanoid::synthetic(ids[1]);
    walk.ticks = WALK_TICKS;
    for ticks in walk.rotation_ticks.chunks_mut(3) {
        ticks.copy_from_slice(&[0, WALK_TICKS / 2, WALK_TICKS]);
    }
    walk.weights.clear();
    walk.weight_ticks.clear();
    walk.weight_keys.clear();
    walk.contacts = contacts.clone();
    let mut run = humanoid::synthetic(ids[2]);
    run.ticks = WALK_TICKS;
    for ticks in run.rotation_ticks.chunks_mut(3) {
        ticks.copy_from_slice(&[0, WALK_TICKS / 2, WALK_TICKS]);
    }
    run.weights.clear();
    run.weight_ticks.clear();
    run.weight_keys.clear();
    run.contacts = contacts;
    run.events = vec![ClipEvent { tick: 10, name: "swish".to_owned() }];
    // The aims bend the spine, one forward and one back.
    let aim = |id: AssetId, angle: f32| {
        let mut clip = humanoid::synthetic(id);
        let key = PackedRotation::encode(sapphire_math::det::quat_from_axis_angle(Vec3::X, angle).to_array());
        clip.rotations = vec![RotationTrack { joint: foot(HumanBone::Spine), count: 2, first: 0 }];
        clip.rotation_ticks = vec![0, clip.ticks];
        clip.rotation_keys = vec![key, key];
        clip
    };
    let (low, high) = (aim(ids[5], -0.6), aim(ids[6], 0.6));
    for (id, clip) in [(ids[1], walk), (ids[2], run), (ids[5], low), (ids[6], high)] {
        let slot = humanoid.clips.iter_mut().find(|(found, _)| *found == id).expect("a clip");
        slot.1 = clip.to_cooked().expect("it cooks");
    }
    humanoid
}

fn feature_binding(humanoid: &Humanoid, upper: &str) -> Arc<GraphBinding> {
    let mut text = features(upper);
    if upper.is_empty() {
        // No layer plays the upper machine, so it goes too.
        let start = text.find(r#"(name: "upper", start"#).expect("the upper machine");
        let end = start + text[start..].find("]),").expect("its end") + 3;
        text.replace_range(start..end, "");
    }
    let mut graph = v2::AnimGraph::from_document(&text).expect("a good document");
    graph.resolve(sapphire_assets::graph::clip_id);
    humanoid.bind(&graph.compile().unwrap_or_else(|error| panic!("{error}")))
}

fn walking(ticks: f64, speed: f32) -> GraphInput {
    GraphInput {
        ticks,
        locomotion: LocomotionInput { speed, ..LocomotionInput::default() },
        ..GraphInput::default()
    }
}

/// A state that plays a machine enters that machine's start; the outer machine's transitions are tried
/// first, and `*` leaves any of its states but the one it enters.
#[test]
fn a_nested_machine_enters_its_start_and_the_outer_machine_leaves_it() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let mut player = GraphPlayer::new(feature_binding(&humanoid, OVER), humanoid.pose(), 1);
    let mut seen: Vec<Name> = Vec::new();
    for frame in 0..120 {
        let grounded = !(10..90).contains(&frame);
        let input = GraphInput {
            ticks: f64::from(frame),
            locomotion: LocomotionInput { grounded, ..LocomotionInput::default() },
            ..GraphInput::default()
        };
        player.evaluate(&input, source.as_slice(), 0);
        if seen.last() != Some(&player.state()) {
            seen.push(player.state());
        }
    }
    assert_eq!(seen, ["ground", "rise", "fall", "ground"].map(Name::new));
}

/// A 1D blend plays its two neighbours linearly between them and its end sample past either end.
#[test]
fn a_one_dimensional_blend_weighs_its_neighbours() {
    let sample = |x: f32| Sample { clip: 0, at: BlendPoint::new(x, 0.0), speed: 0.0, phase: 0.0, sync: true };
    let samples = [sample(-1.0), sample(0.0), sample(2.0)];
    let weights = |value: f32| {
        let mut weights = [9.0; MAX_BLEND_SAMPLES];
        line_weights(&samples, value, &mut weights);
        weights[..3].to_vec()
    };
    assert_eq!(weights(-5.0), [1.0, 0.0, 0.0]);
    assert_eq!(weights(-1.0), [1.0, 0.0, 0.0]);
    assert_eq!(weights(-0.25), [0.25, 0.75, 0.0]);
    assert_eq!(weights(0.0), [0.0, 1.0, 0.0]);
    assert_eq!(weights(1.5), [0.0, 0.25, 0.75]);
    assert_eq!(weights(9.0), [0.0, 0.0, 1.0]);
}

/// A machine layer over the upper body by a mask: the game's slot moves its blend, under the mask only;
/// a slot the graph doesn't have is refused.
#[test]
fn a_masked_machine_layer_plays_a_game_slots_blend_over_the_upper_body() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let binding = feature_binding(&humanoid, OVER);
    let pose_at = |aim: f32| {
        let mut player = GraphPlayer::new(Arc::clone(&binding), humanoid.pose(), 1);
        assert!(player.set_game_slot(Name::new("aim"), aim));
        assert!(!player.set_game_slot(Name::new("nothing"), aim));
        for frame in 0..20 {
            player.evaluate(&walking(f64::from(frame), 1.5), source.as_slice(), 0);
        }
        player.pose().locals().to_vec()
    };
    let (low, high) = (pose_at(-1.0), pose_at(1.0));
    let index = |bone: HumanBone| HumanBone::ALL.iter().position(|&found| found == bone).expect("a bone");
    let hips = index(HumanBone::Hips);
    let spine = index(HumanBone::Spine);
    assert_eq!(
        transform_bits(&low[hips..=hips]),
        transform_bits(&high[hips..=hips]),
        "the hips are the base's"
    );
    assert_ne!(transform_bits(&low[spine..]), transform_bits(&high[spine..]), "the upper body is the slot's");
}

/// An additive machine layer adds each clip's difference from its own first tick: nothing while every
/// clip stands at its first tick, its motion after.
#[test]
fn an_additive_machine_layer_adds_each_clips_difference_from_its_first_tick() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let added = feature_binding(&humanoid, ADDED);
    let without = feature_binding(&humanoid, "");
    let mut players = [added, without].map(|graph| GraphPlayer::new(graph, humanoid.pose(), 1));
    let mut differed = false;
    for frame in 0..40 {
        for player in &mut players {
            player.set_game_slot(Name::new("aim"), 0.3);
            player.evaluate(&walking(f64::from(frame), 1.5), source.as_slice(), 0);
        }
        let same = transform_bits(players[0].pose().locals()) == transform_bits(players[1].pose().locals());
        if frame == 0 {
            assert!(same, "at the clips' first tick the layer adds nothing");
        }
        differed |= !same;
    }
    assert!(differed, "and its motion after");
}

/// A synced clip counts its cycle from the first tick its group's foot makes the group's marker.
#[test]
fn a_sync_group_counts_the_cycle_from_its_foots_marker() {
    let humanoid = feature_humanoid();
    let binding = feature_binding(&humanoid, OVER);
    let source = humanoid.source();
    let walk = source[1].1;
    let at = |phase: f32| binding.synced_position(walk, Some(0), phase);
    assert_eq!(at(0.0), f32::from(LEFT_DOWN));
    assert_eq!(at(0.5), f32::from(LEFT_DOWN + WALK_TICKS / 2));
    assert_eq!(
        at(0.9),
        (0.9 * f32::from(WALK_TICKS) + f32::from(LEFT_DOWN)).rem_euclid(f32::from(WALK_TICKS))
    );
    assert_eq!(binding.synced_position(walk, None, 0.5), 0.5 * f32::from(WALK_TICKS), "no group, no offset");
}

/// Events fire on the frame a clip weighing enough passes their marker or tick, each once, and a frame
/// with nothing to do sends none.
#[test]
fn events_fire_when_a_clip_weighing_enough_passes_them() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let mut player = GraphPlayer::new(feature_binding(&humanoid, OVER), humanoid.pose(), 1);
    let footstep = Name::new("footstep");
    let swish = Name::new("swish");
    let mut steps = 0;
    let frames = 3 * u32::from(WALK_TICKS);
    for frame in 0..frames {
        player.evaluate(&walking(f64::from(frame), 1.5), source.as_slice(), 0);
        assert!(!player.events().contains(&swish), "the run weighs nothing at a walk");
        steps += usize::from(player.events().contains(&footstep));
        // The clock stands still: nothing to do, nothing sent.
        player.evaluate(&walking(f64::from(frame), 1.5), source.as_slice(), 0);
        assert!(player.events().is_empty());
    }
    assert!((2..=3).contains(&steps), "a left footstep a cycle: {steps} over three");
    let mut swishes = 0;
    for frame in frames..frames + 60 {
        player.evaluate(&walking(f64::from(frame), 5.0), source.as_slice(), 0);
        swishes += usize::from(player.events().contains(&swish));
    }
    assert!(swishes >= 1, "at a run its clip's own event fires");
    assert_eq!(player.dropped_events(), 0);
}

/// The feature graph's text, for `target`: compiled for the fixed step, its game slot reads a locomotion
/// value instead and it has no events, which only presentation runs.
fn feature_text(upper: &str, target: Target) -> String {
    let mut text = features(upper);
    if upper.is_empty() {
        // No layer plays the upper machine, so it goes too.
        let start = text.find(r#"(name: "upper", start"#).expect("the upper machine");
        let end = start + text[start..].find("]),").expect("its end") + 3;
        text.replace_range(start..end, "");
    }
    if target == Target::Simulation {
        text = text.replacen(r#"from: Game("aim")"#, "from: Locomotion(SmoothedSpeed)", 1);
        let start = text.find("events: [").expect("the events");
        let end = start + text[start..].find("],").expect("their end") + 2;
        text.replace_range(start..end, "");
    }
    text
}

fn feature_program(text: &str, target: Target) -> v2::GraphProgram {
    let mut graph = v2::AnimGraph::from_document(text).expect("a good document");
    graph.resolve(sapphire_assets::graph::clip_id);
    graph.compile_for(target).unwrap_or_else(|error| panic!("{error}"))
}

/// The upper layer's slot.
fn upper_slot(player: &GraphPlayer) -> usize {
    (0..MAX_LAYERS).find(|&slot| player.graph.has_layer(slot)).expect("a machine layer")
}

/// The upper layer's state, by name.
fn upper_state(player: &GraphPlayer) -> Name {
    player.graph.states[usize::from(player.layers.layer_states[upper_slot(player)].state)].name
}

/// A layer's clock starts on the frame the actor is first evaluated, as the base's does, and again on a
/// seek back: a one-shot layer bound late plays out before it moves on.
#[test]
fn a_layer_first_evaluated_late_plays_its_one_shot_from_that_frame() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let one_shot = features(OVER)
        .replacen(r#"(name: "upper", start: "aim""#, r#"(name: "upper", start: "draw""#, 1)
        .replacen(
            r#"(name: "aim", plays: Blend1D((parameter: "aim", samples: [(clip: "aim_low", at: -1.0), (clip: "aim_high", at: 1.0)]))),
            ]),"#,
            r#"(name: "draw", plays: Clip(clip: "aim_low", looping: false)),
                (name: "hold", plays: Clip(clip: "aim_high", looping: true)),
            ], transitions: [(from: "draw", to: "hold", when: "finished", decay: 2)]),"#,
            1,
        );
    assert!(one_shot.contains(r#"(name: "draw""#), "the one-shot replaced the aim");
    let binding = humanoid.bind(&feature_program(&one_shot, Target::Presentation));
    let (draw, hold) = (Name::new("draw"), Name::new("hold"));
    // How many frames the draw plays from the first.
    let drawn_for = |first: u32| {
        let mut player = GraphPlayer::new(Arc::clone(&binding), humanoid.pose(), 1);
        for frame in 0..200 {
            player.evaluate(&walking(f64::from(first + frame), 0.0), source.as_slice(), 0);
            if upper_state(&player) == hold {
                return (frame, player);
            }
            assert_eq!(upper_state(&player), draw);
        }
        panic!("the draw never finished");
    };
    let (from_zero, _) = drawn_for(0);
    let (from_late, mut player) = drawn_for(600);
    assert!(from_zero > 1, "the draw plays for {from_zero} frames");
    assert_eq!(from_late, from_zero, "bound at tick 600 it plays as long as from tick 0");
    // A seek back keeps the layer's state and starts its clock again there.
    player.evaluate(&walking(610.0, 0.0), source.as_slice(), 0);
    assert_eq!(upper_state(&player), hold);
    assert_eq!(player.layers.layer_states[upper_slot(&player)].started, 610.0);
}

fn feature_input(frame: u32, cut: bool) -> GraphInput {
    let speed = [0.0, 1.5, 3.0, 5.0][(frame / 40) as usize % 4];
    GraphInput {
        ticks: f64::from(frame),
        cut,
        locomotion: LocomotionInput {
            speed,
            grounded: !(120..140).contains(&frame),
            ..LocomotionInput::default()
        },
        ..GraphInput::default()
    }
}

fn feature_stamp(frame: u32, generation: u32) -> GraphStepStamp {
    GraphStepStamp {
        step: u64::from(frame),
        owner_now: SimTime::from_subticks(u64::from(frame) * u64::from(SUBTICKS_PER_TICK)),
        generation,
    }
}

/// v2's own features on the fixed step's maths, through two histories: an override and an additive
/// machine layer, a nested machine, a 1D blend and a sync group, with a jump and a cut. The two agree
/// each step, and the checksum over the session is pinned. The pins were recorded on arm64 and hold
/// for every architecture: CI's Linux and Windows runs prove them on x86-64, and a mismatch there is a
/// determinism finding to chase, never a per-architecture value to record.
#[test]
fn the_feature_graph_runs_the_fixed_step_the_same_every_time() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let mut hashes = Vec::new();
    for upper in [OVER, ADDED] {
        let text = feature_text(upper, Target::Simulation);
        let binding = humanoid.bind(&feature_program(&text, Target::Simulation));
        assert!(binding.has_machine_layers());
        let mut histories = [0, 1].map(|_| {
            FixedGraphHistory::try_new(Arc::clone(&binding), humanoid.pose(), 11, source.as_slice())
                .expect("it runs fixed")
        });
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut generation = 0;
        for frame in 0..240_u32 {
            let input = feature_input(frame, frame == 200);
            generation += u32::from(input.cut);
            let mut sums = [Vec::new(), Vec::new()];
            for (history, sum) in histories.iter_mut().zip(&mut sums) {
                history.advance(feature_stamp(frame, generation), input, None).expect("a step forward");
                history.write_canonical(&mut |chunk: &[u8]| sum.extend_from_slice(chunk));
            }
            assert_eq!(sums[0], sums[1], "step {frame}");
            for &byte in &sums[0] {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
            }
        }
        hashes.push(hash);
    }
    assert_eq!(
        hashes,
        [0xf556_f4a5_4984_1228, 0x6b97_5473_f503_897e],
        "the fixed step's checksum over the session moved: {hashes:#018x?}"
    );
}

/// The feature graph allocates nothing once its players are made: override and additive layers, a
/// nested machine, a 1D blend on a game slot, a sync group, events and a replacement on presentation's
/// maths, and the same graph compiled for the fixed step.
#[test]
fn the_feature_graph_plays_with_no_allocation() {
    use sapphire_core::alloc::{self, PER_THREAD_COUNTS};
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let clips = source.as_slice();
    let mut players = [OVER, ADDED].map(|upper| {
        let text = feature_text(upper, Target::Presentation);
        let binding = humanoid.bind(&feature_program(&text, Target::Presentation));
        (GraphPlayer::new(Arc::clone(&binding), humanoid.pose(), 1), binding)
    });
    let text = feature_text(OVER, Target::Simulation);
    let simulation = humanoid.bind(&feature_program(&text, Target::Simulation));
    let mut history =
        FixedGraphHistory::try_new(simulation, humanoid.pose(), 11, clips).expect("it runs fixed");
    let (fired, counts) = alloc::count_in_scope(|| {
        let mut fired = 0;
        for frame in 0..240_u32 {
            let input = feature_input(frame, false);
            for (player, binding) in &mut players {
                if frame == 150 {
                    player.replace(Arc::clone(binding));
                }
                player.set_game_slot(Name::new("aim"), (frame % 120) as f32 / 60.0 - 1.0);
                player.evaluate(&input, clips, 0);
                player.finish();
                fired += player.events().len();
            }
            history.advance(feature_stamp(frame, 0), input, None).expect("a step forward");
        }
        fired
    });
    if PER_THREAD_COUNTS {
        assert_eq!(counts.allocations, 0, "{counts:?}");
    }
    assert!(fired > 0, "the session sends events");
}

/// How many frames of three walk cycles send a left footstep, with `move_name` playing throughout if
/// given.
fn footsteps(binding: &Arc<GraphBinding>, humanoid: &Humanoid, move_name: Option<&str>) -> usize {
    let source = humanoid.source();
    let mut player = GraphPlayer::new(Arc::clone(binding), humanoid.pose(), 1);
    let footstep = Name::new("footstep");
    (0..3 * u32::from(WALK_TICKS))
        .filter(|&frame| {
            let input = GraphInput {
                combat: move_name.map(|name| CombatInput {
                    name: Name::new(name),
                    serial: 1,
                    position: f64::from(frame),
                }),
                ..walking(f64::from(frame), 1.5)
            };
            player.evaluate(&input, source.as_slice(), 0);
            player.events().contains(&footstep)
        })
        .count()
}

/// An override layer hides the feet it lays over from events, as it does from the pose: a whole-body
/// layer at full weight sends none of the walk's footsteps, and a move masked to the legs none either,
/// while the upper body's layer leaves them.
#[test]
fn an_override_layer_or_a_masked_move_hides_the_feet_below_it_from_events() {
    let humanoid = feature_humanoid();
    let program = |upper: &str| {
        let text = features(upper).replacen(
            r#"masks: [(name: "upper", bones: [(bone: Spine, weight: 1.0)])],"#,
            r#"masks: [(name: "upper", bones: [(bone: Spine, weight: 1.0)]),
                (name: "legs", bones: [(bone: LeftUpperLeg, weight: 1.0), (bone: RightUpperLeg, weight: 1.0)])],"#,
            1,
        );
        assert!(text.contains(r#""legs""#));
        humanoid.bind(&feature_program(&text, Target::Presentation))
    };
    let shown = footsteps(&program(OVER), &humanoid, None);
    assert!(shown >= 2, "the upper body's layer leaves the feet: {shown}");
    let whole = r#"(name: "upper", plays: Machine((machine: "upper", weight: 1.0))),"#;
    assert_eq!(footsteps(&program(whole), &humanoid, None), 0, "a whole-body layer hides them");
    let kick = format!(
        r#"{OVER}
            (name: "combat", plays: Combat((decay: 5, exit: 12, moves: [(move: "kick", clip: "rise", mask: Some("legs"))]))),"#
    );
    let binding = program(&kick);
    assert_eq!(footsteps(&binding, &humanoid, None), shown, "no move, the walk's feet");
    assert_eq!(footsteps(&binding, &humanoid, Some("kick")), 0, "a move over the legs hides them");
}
