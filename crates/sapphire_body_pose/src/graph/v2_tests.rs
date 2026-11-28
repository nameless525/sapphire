//! Graph v2's runtime on the shipped graphs and on what only version 2 says (M6 A5b, A5c).
//!
//! The graphs shipped at the switch, frozen as this crate's test data (`humanoid::FROZEN`) and compiled
//! for the fixed step as the cook compiles them, play what version 1's player played before the switch,
//! through a scripted session that reaches each of their states, transitions and rules and every reset
//! of the smoothed velocity: [`PLAYED`] holds it, taken from version 1's player while it still ran (A5b
//! held the two bit for bit on both maths, frame for frame). The pins guard the runtime: the frozen
//! copies never change with the content, so a new graph or an edited one never touches them. The graphs
//! shipped now are only compiled for the fixed step and bound
//! ([`every_shipped_graph_binds_to_the_fixed_step`]), read from the content tree, never named: a boss's
//! graph is content (gameplay's `bosses_are_content`).
//!
//! What the pins hold: the fixed step's checksum bytes each step (its pose, weights, target and history
//! on the deterministic maths), and presentation's clips sampled, state name, stats, teleports and foot
//! contact each frame and what it reached. They don't hold presentation's pose, weights or target, on
//! host maths: only the goldens catch those.
//!
//! The clips are synthetic, keyed by each graph's own clip IDs on a humanoid skeleton with every bone,
//! so no cooked content is needed.

use std::sync::Arc;

use sapphire_assets::AssetId;
use sapphire_core::Name;
use sapphire_time::{SUBTICKS_PER_TICK, SimTime};

use super::*;

use crate as body;
#[path = "../../tests/support/humanoid.rs"]
mod humanoid;
use humanoid::{Humanoid, Step, frozen, read, session, shipped};
use sapphire_assets::clip::{PackedRotation, RotationTrack};
use sapphire_assets::graph::v2::GraphProgram;
use sapphire_assets::rig::HumanBone;
use sapphire_math::Vec3;

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

/// What a binding played through a session.
struct Played {
    /// The fixed step's checksum bytes each step (pose, weights, target and the whole history, on the
    /// deterministic maths), and presentation's clips sampled, state, stats, teleports and foot contact
    /// each frame and what it reached: two digests.
    digests: (u64, u64),
    /// Presentation's clips sampled each frame.
    samples: Vec<u64>,
    /// What presentation's player reached.
    reached: u64,
}

/// Plays `steps` on `binding` in the fixed step and in presentation.
fn played(humanoid: &Humanoid, binding: &Arc<GraphBinding>, steps: &[Step]) -> Played {
    let source = humanoid.source();
    let clips = source.as_slice();
    let mut history = FixedGraphHistory::try_new(Arc::clone(binding), humanoid.pose(), 11, clips)
        .unwrap_or_else(|error| panic!("{error}"));
    let mut player = GraphPlayer::new(Arc::clone(binding), humanoid.pose(), 11);
    let (mut fixed, mut shown) = (Digest::new(), Digest::new());
    let mut samples = Vec::with_capacity(steps.len());
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
        samples.push(sampled);
    }
    shown.write(&player.reached.to_le_bytes());
    Played { digests: (fixed.0, shown.0), samples, reached: player.reached }
}

/// A frozen graph, read, compiled for the fixed step as the cook compiles it and bound.
struct Shipped {
    name: String,
    humanoid: Humanoid,
    program: GraphProgram,
    binding: Arc<GraphBinding>,
    steps: Vec<Step>,
}

/// Every frozen graph, as [`Shipped`], in `humanoid::FROZEN`'s order.
fn frozen_bindings() -> Vec<Shipped> {
    frozen()
        .into_iter()
        .map(|(name, text)| {
            let graph = read(&text);
            let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
            let program =
                graph.compile_for(Target::Simulation).unwrap_or_else(|error| panic!("{name}: {error}"));
            let binding = humanoid.bind(&program);
            let steps = session(&graph);
            Shipped { name, humanoid, program, binding, steps }
        })
        .collect()
}

/// What each frozen graph plays through the session, in `humanoid::FROZEN`'s order: [`played`]'s two
/// digests, taken from version 1's player before the switch (M6 A5c). They move only with the runtime:
/// a change that moves one on purpose says why and re-pins its row; a content edit never does.
const PLAYED: [(u64, u64); 7] = [
    (0x6672_79b6_4c92_18b5, 0xc621_2f18_d6c2_afc6),
    (0x5ef1_844f_2257_309e, 0x1287_50ea_9c42_cfeb),
    (0x677d_a669_6679_be33, 0x60b0_7dde_1999_5b2e),
    (0x41d3_096e_1ec1_0102, 0x635a_f86c_e5f1_1c7f),
    (0xe154_eb74_2d99_313d, 0x8a25_ea98_e032_317a),
    (0x4493_f9c4_5842_eab6, 0x9241_2dae_1049_254a),
    (0xb401_d98f_b0c1_6085, 0xe2ae_71cc_0f1a_1f9f),
];

/// A5c's bar, kept once v1 is gone: each frozen graph, read, compiled for the fixed step and bound as
/// the game binds it, plays what version 1's player played, through a session that reaches every state,
/// transition, rule and reset of the smoothed velocity.
#[test]
fn every_frozen_graph_plays_what_version_1_played() {
    let shipped = frozen_bindings();
    let mut found = Vec::new();
    for Shipped { name, humanoid, binding, steps, .. } in &shipped {
        let played = played(humanoid, binding, steps);
        must_reach(name, binding, played.reached);
        found.push(played.digests);
    }
    assert_eq!(found, PLAYED, "{found:#x?}");
}

/// Every graph in the content tree compiles for the fixed step, as the cook compiles it, and binds there:
/// what a content edit must keep, with no pinned bytes.
#[test]
fn every_shipped_graph_binds_to_the_fixed_step() {
    for (name, text) in shipped() {
        let graph = read(&text);
        let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
        let program = graph.compile_for(Target::COOKED).unwrap_or_else(|error| panic!("{name}: {error}"));
        let source = humanoid.source();
        FixedGraphHistory::try_new(humanoid.bind(&program), humanoid.pose(), 0, source.as_slice())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

/// A program compiled for presentation may hold what only presentation runs, so the fixed step
/// refuses it; the same graph compiled for the fixed step, as the cook compiles it, runs there.
#[test]
fn the_fixed_step_refuses_a_program_compiled_for_presentation() {
    let (_, text) = frozen().into_iter().find(|(name, _)| name == "heroine").expect("the heroine is frozen");
    let graph = read(&text);
    let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
    let source = humanoid.source();
    let presentation = humanoid.bind(&graph.compile().expect("compiles"));
    assert_eq!(presentation.target(), Target::Presentation);
    let refused = FixedGraphHistory::try_new(presentation, humanoid.pose(), 0, source.as_slice());
    assert!(matches!(refused, Err(GraphBindError::PresentationProgram)));
    let simulation = humanoid.bind(&graph.compile_for(Target::Simulation).expect("compiles"));
    assert_eq!(simulation.target(), Target::Simulation);
    assert!(FixedGraphHistory::try_new(simulation, humanoid.pose(), 0, source.as_slice()).is_ok());
}

/// Prints what each frozen graph costs a frame through the session, and asserts nothing: clips sampled
/// (the mean and the worst frame; [`PLAYED`]'s presentation digest holds each frame's count, so they are
/// what version 1's player sampled), and the program's own worst case an evaluation.
#[test]
#[allow(clippy::print_stdout, reason = "the cost table is what it's for")]
fn print_what_each_frozen_graph_costs() {
    let mut table = Vec::new();
    for Shipped { name, humanoid, program, binding, steps } in frozen_bindings() {
        let samples = played(&humanoid, &binding, &steps).samples;
        let worst = samples.iter().copied().max().unwrap_or(0);
        let mean = samples.iter().sum::<u64>() as f64 / samples.len() as f64;
        let cost = program.cost;
        table.push(format!(
            "{name}: {} frames, clips sampled a frame {mean:.2} mean and {worst} worst; an evaluation's \
             worst case {} clips, {} registers, {} ops, {} condition ops",
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

fn feature_program(text: &str, target: Target) -> GraphProgram {
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

/// A snapshot (M6 A9) says each machine layer's state, how long it has played and its blend's weights, every
/// parameter by name and value, and the move playing, as of the frame drawn.
#[test]
fn a_snapshot_says_each_layers_state_and_weights_and_every_parameter() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let mut player = GraphPlayer::new(feature_binding(&humanoid, OVER), humanoid.pose(), 1);
    let mut snapshot = GraphSnapshot::new();
    player.debug_snapshot(&mut snapshot);
    assert_eq!(snapshot.drawn, None, "nothing drawn yet");
    player.set_game_slot(Name::new("aim"), 0.3);
    for frame in 0..20 {
        let input = GraphInput {
            combat: (frame >= 12).then_some(CombatInput {
                name: Name::new("kick"),
                serial: 1,
                position: f64::from(frame - 12),
            }),
            ..walking(f64::from(frame), 1.5)
        };
        player.evaluate(&input, source.as_slice(), 0);
    }
    player.debug_snapshot(&mut snapshot);
    assert_eq!(snapshot.drawn, Some(19.0));
    assert_eq!(snapshot.combat, None, "the graph has no row for the move");
    let layers = snapshot.layers();
    assert_eq!(layers.len(), 2, "{layers:?}");
    assert_eq!((layers[0].machine, layers[0].state), (Name::new("locomotion"), Name::new("ground")));
    assert_eq!(layers[0].time, 19.0);
    assert_eq!(layers[0].weights().len(), 3);
    assert!((layers[0].weights().iter().sum::<f32>() - 1.0).abs() < 1e-5, "{layers:?}");
    assert!(layers[0].weights()[1] > layers[0].weights()[2], "walking at 1.5 m/s is mostly the walk");
    assert_eq!((layers[1].machine, layers[1].state), (Name::new("upper"), Name::new("aim")));
    let [low, high] = layers[1].weights() else { panic!("{layers:?}") };
    assert!((low - 0.35).abs() < 1e-5 && (high - 0.65).abs() < 1e-5, "{layers:?}");
    let names: Vec<Name> = snapshot.parameters().iter().map(|parameter| parameter.name).collect();
    // The graph's own, then the controller's inputs it doesn't name.
    let own = ["velocity_x", "velocity_z", "grounded", "aim"];
    let fed = ["speed", "direction", "landed", "preparing_jump"];
    assert_eq!(names, own.into_iter().chain(fed).map(Name::new).collect::<Vec<_>>());
    let value = |name: &str| snapshot.parameter(Name::new(name)).map(|parameter| parameter.value);
    assert_eq!((value("grounded"), value("aim"), value("speed")), (Some(1.0), Some(0.3), Some(1.5)));
    assert!(snapshot.parameters().iter().all(|parameter| !parameter.overridden));
    // Every name it shows has its text, in any build.
    let label = |name: Name| player.label(name);
    assert_eq!((label(layers[1].machine), label(layers[1].state)), (Some("upper"), Some("aim")));
    assert_eq!((label(Name::new("aim")), label(Name::new("speed"))), (Some("aim"), Some("speed")));
    assert_eq!(label(Name::new("height")), None);
}

/// An override (M6 A9) is what the graph reads, from the next frame and on a frozen clock too, until it's
/// cleared: one read from the controller in the input, a game slot in its register. A parameter the graph
/// doesn't have, or a value that isn't a number, is refused and pins nothing.
#[test]
fn an_override_is_what_the_graph_reads_until_its_cleared() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let mut player = GraphPlayer::new(feature_binding(&humanoid, OVER), humanoid.pose(), 1);
    let mut snapshot = GraphSnapshot::new();
    let mut frame = 0.0;
    let mut play = |player: &mut GraphPlayer, frames: u32| {
        for _ in 0..frames {
            player.evaluate(&walking(frame, 1.5), source.as_slice(), 0);
            frame += 1.0;
        }
        frame - 1.0
    };
    let last = play(&mut player, 10);
    assert_eq!(player.state(), Name::new("ground"));
    assert_eq!(
        player.set_override(Name::new("height"), 1.0),
        Err(OverrideError::NoSuchParameter(Name::new("height")))
    );
    assert_eq!(player.set_override(Name::new("grounded"), f32::NAN), Err(OverrideError::NotANumber));
    assert_eq!(player.overrides().count(), 0);

    // On a frozen clock the graph takes it at once, since the key covers the overrides; the pose keeps
    // its bits and decays toward the new state when the clock moves.
    player.set_override(Name::new("grounded"), 0.0).expect("the graph has it");
    player.set_override(Name::new("aim"), -1.0).expect("a game slot");
    player.evaluate(&walking(last, 1.5), source.as_slice(), 0);
    assert_eq!(
        player.state(),
        Name::new("rise"),
        "in the air for the graph, on the ground for the controller"
    );
    play(&mut player, 70);
    assert_eq!(player.state(), Name::new("fall"));
    player.debug_snapshot(&mut snapshot);
    let grounded = snapshot.parameter(Name::new("grounded")).expect("there");
    assert_eq!((grounded.value, grounded.overridden), (0.0, true));
    assert_eq!(snapshot.layers()[1].weights(), [1.0, 0.0], "the aim pinned low");
    // Setting it again replaces it; a truth is 1 for anything but 0.
    player.set_override(Name::new("grounded"), 0.25).expect("again");
    assert_eq!(
        player.overrides().collect::<Vec<_>>(),
        [(Name::new("grounded"), 1.0), (Name::new("aim"), -1.0)]
    );
    play(&mut player, 2);
    assert_eq!(player.state(), Name::new("ground"));

    player.set_override(Name::new("grounded"), 0.0).expect("again");
    play(&mut player, 2);
    assert_eq!(player.state(), Name::new("rise"));
    player.clear_overrides();
    play(&mut player, 2);
    assert_eq!(player.state(), Name::new("ground"), "what the controller says, again");
    // The controller's speed, which the graph doesn't name, is the input the smoothed velocity follows.
    player.set_override(Name::new("speed"), 0.0).expect("every graph is fed it");
    play(&mut player, 60);
    player.debug_snapshot(&mut snapshot);
    let speed = snapshot.parameter(Name::new("speed")).expect("there");
    assert_eq!((speed.value, speed.overridden), (0.0, true));
    let forward = snapshot.parameter(Name::new("velocity_z")).expect("there").value;
    assert!(forward.abs() < 0.05, "the smoothed velocity settled at the overridden speed: {forward}");
    player.clear_overrides();
    play(&mut player, 2);
    player.debug_snapshot(&mut snapshot);
    assert!(snapshot.parameters().iter().all(|parameter| !parameter.overridden));
    assert_eq!(snapshot.parameter(Name::new("aim")).map(|parameter| parameter.value), Some(0.0));
    // A held frame has no time to smooth over, so the smoothed velocity takes an overridden speed at once.
    let held = play(&mut player, 60);
    player.debug_snapshot(&mut snapshot);
    let walking_at = snapshot.parameter(Name::new("velocity_z")).expect("there").value;
    assert!(walking_at.abs() > 0.5, "walking again: {walking_at}");
    player.set_override(Name::new("speed"), 0.0).expect("every graph is fed it");
    player.evaluate(&walking(held, 1.5), source.as_slice(), 0);
    player.debug_snapshot(&mut snapshot);
    let forward = snapshot.parameter(Name::new("velocity_z")).expect("there").value;
    assert!(forward.abs() < 1e-6, "the held frame took the overridden speed: {forward}");
}

/// What a person pinned holds for a new version of the character (`continue_from`, for a new mesh, rig or
/// skeleton), as through a replacement, and `check_override` says what `set_override` would, changing nothing.
#[test]
fn an_override_holds_for_a_new_version_of_the_character() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let mut old = GraphPlayer::new(feature_binding(&humanoid, OVER), humanoid.pose(), 1);
    assert_eq!(
        old.check_override(Name::new("height"), 1.0),
        Err(OverrideError::NoSuchParameter(Name::new("height")))
    );
    assert_eq!(old.check_override(Name::new("speed"), f32::INFINITY), Err(OverrideError::NotANumber));
    assert_eq!(old.check_override(Name::new("speed"), 0.0), Ok(()));
    assert_eq!(old.overrides().count(), 0, "a check pins nothing");
    old.set_override(Name::new("aim"), 1.0).expect("a game slot");
    old.set_override(Name::new("speed"), 0.0).expect("the controller's");
    old.evaluate(&walking(0.0, 1.5), source.as_slice(), 0);
    let mut new = GraphPlayer::new(feature_binding(&humanoid, ADDED), humanoid.pose(), 1);
    new.continue_from(&old);
    assert_eq!(new.overrides().collect::<Vec<_>>(), [(Name::new("aim"), 1.0), (Name::new("speed"), 0.0)]);
    new.evaluate(&walking(1.0, 1.5), source.as_slice(), 0);
    let mut snapshot = GraphSnapshot::new();
    new.debug_snapshot(&mut snapshot);
    let aim = snapshot.parameter(Name::new("aim")).expect("there");
    assert_eq!((aim.value, aim.overridden), (1.0, true));
}

/// An override holds through a replacement graph by name: once the new graph is adopted, it pins the same
/// parameter there.
#[test]
fn an_override_holds_through_a_replacement_by_name() {
    let humanoid = feature_humanoid();
    let source = humanoid.source();
    let binding = feature_binding(&humanoid, OVER);
    let mut player = GraphPlayer::new(Arc::clone(&binding), humanoid.pose(), 1);
    player.set_override(Name::new("aim"), 1.0).expect("a game slot");
    player.replace(feature_binding(&humanoid, ADDED));
    for frame in 0..40 {
        // In the air and down again: a state's start adopts the replacement.
        let grounded = !(10..20).contains(&frame);
        let input = GraphInput {
            ticks: f64::from(frame),
            locomotion: LocomotionInput { grounded, ..LocomotionInput::default() },
            ..GraphInput::default()
        };
        player.evaluate(&input, source.as_slice(), 0);
    }
    assert!(!player.has_replacement(), "adopted");
    let mut snapshot = GraphSnapshot::new();
    player.debug_snapshot(&mut snapshot);
    assert_eq!(snapshot.revision, 1);
    let aim = snapshot.parameter(Name::new("aim")).expect("there");
    assert_eq!((aim.value, aim.overridden), (1.0, true));
}
