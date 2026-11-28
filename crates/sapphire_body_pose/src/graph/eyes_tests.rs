//! The eyes (M6 A22): a pure function of the owner's clock and the character's seed, through either kind
//! of VRM look-at, turned down by the face's overrides, never in the fixed step, a large jump sometimes
//! with a blink.

use std::sync::Arc;

use sapphire_assets::graph::v2::{self, GraphProgram, Target};
use sapphire_assets::rig::{
    CharacterRig, Expression, ExpressionOverride, ExpressionPreset, HumanBone, LookAtKind, MorphTargetWeight,
};
use sapphire_math::Vec3;

use super::*;

// The humanoid comes from the runtime's tests, since a file is a module once in a crate.
use super::v2_tests::humanoid::{self, Humanoid};

/// The look expressions' morph targets on the expression rig, after the humanoid's three.
const LOOK_LEFT: usize = 3;
const LOOK_RIGHT: usize = 4;
const LOOK_UP: usize = 5;
const LOOK_DOWN: usize = 6;
const MORPHS: usize = 7;
const BLINK: usize = 1;

const REACH: [f32; 2] = [30.0, 15.0];
const HOLD: [u32; 2] = [40, 150];
const SPEED: f32 = 3.0;

fn source() -> String {
    "00000000-0000-4000-8000-000000000001".to_owned()
}

/// One idle state, a blink if `blink` says one, and eyes with `eyes` as their document says them.
fn text(eyes: &str, blink: &str) -> String {
    format!(
        r#"(
    type: "sapphire::assets::AnimGraph",
    version: 2,
    data: (
        clips: [(name: "idle", source: "{source}")],
        machines: [(name: "locomotion", start: "stand", states: [(name: "stand", plays: Clip(clip: "idle", looping: true))])],
        layers: [(name: "locomotion", plays: Machine((machine: "locomotion")))],
        {blink}
        {eyes}
    ),
)"#,
        source = source(),
    )
}

fn eyes_of(chance: f32) -> String {
    format!(
        "eyes: Some((reach: ({}, {}), hold: ({}, {}), speed: {SPEED}, blink_chance: {chance})),",
        REACH[0], REACH[1], HOLD[0], HOLD[1]
    )
}

fn program(text: &str, target: Target) -> GraphProgram {
    let mut graph = v2::AnimGraph::from_document(text).expect("a good document");
    graph.resolve(sapphire_assets::graph::clip_id);
    graph.compile_for(target).unwrap_or_else(|error| panic!("{error}"))
}

/// The humanoid, its look-at turning eye bones (`Bone`) or weighing look expressions (`Expression`).
struct Character {
    humanoid: Humanoid,
    morphs: usize,
}

impl Character {
    fn new(kind: LookAtKind) -> Self {
        let mut humanoid = Humanoid::new([source().parse().expect("an asset ID")]);
        if kind == LookAtKind::Bone {
            return Self { humanoid, morphs: humanoid::MORPHS };
        }
        let mut rig = humanoid::humanoid_rig();
        rig.morph_target_count = MORPHS as u32;
        let expression = |preset: ExpressionPreset, target: usize| Expression {
            name: preset.name().to_owned(),
            preset: Some(preset),
            binary: false,
            override_blink: ExpressionOverride::None,
            override_look_at: ExpressionOverride::None,
            override_mouth: ExpressionOverride::None,
            morph_targets: vec![MorphTargetWeight { target: target as u32, weight: 1.0 }],
            material_colors: Vec::new(),
            texture_transforms: Vec::new(),
        };
        rig.expressions.extend([
            expression(ExpressionPreset::LookUp, LOOK_UP),
            expression(ExpressionPreset::LookDown, LOOK_DOWN),
            expression(ExpressionPreset::LookLeft, LOOK_LEFT),
            expression(ExpressionPreset::LookRight, LOOK_RIGHT),
        ]);
        // As the shipped heroes': full weight at 30° of gaze.
        let range = sapphire_assets::rig::LookAtRange { input_max_degrees: 30.0, output_scale: 1.0 };
        rig.look_at = rig.look_at.map(|look| sapphire_assets::rig::LookAt {
            kind: LookAtKind::Expression,
            horizontal_inner: range,
            horizontal_outer: range,
            vertical_down: range,
            vertical_up: range,
            ..look
        });
        humanoid.rig = rig.to_cooked().expect("it cooks");
        Self { humanoid, morphs: MORPHS }
    }

    fn bind(&self, program: &GraphProgram) -> Arc<GraphBinding> {
        let source = self.humanoid.source();
        let (skeleton, rig) = (self.humanoid.skeleton(), self.humanoid.rig());
        Arc::new(
            GraphBinding::from_program(program, skeleton, Some(rig), self.morphs, source.as_slice())
                .unwrap_or_else(|error| panic!("{error}")),
        )
    }

    fn player(&self, graph: &Arc<GraphBinding>, seed: u32) -> GraphPlayer {
        let mut rest = vec![0.0; self.morphs];
        rest[2] = 0.25;
        GraphPlayer::new(graph.clone(), CharacterPose::new(self.humanoid.skeleton(), &[], &[], rest), seed)
    }

    fn eye(&self, bone: HumanBone) -> usize {
        HumanBone::ALL.iter().position(|&found| found == bone).expect("every bone")
    }
}

fn idle(ticks: f64) -> GraphInput {
    GraphInput { ticks, ..GraphInput::default() }
}

/// What the eyes show at a frame: both eye bones' rotations and every morph weight, as bits.
fn eyes_shown(character: &Character, player: &GraphPlayer) -> Vec<u32> {
    let pose = player.pose();
    let mut bits: Vec<u32> = [HumanBone::LeftEye, HumanBone::RightEye]
        .into_iter()
        .flat_map(|bone| pose.locals()[character.eye(bone)].rotation.to_array().map(f32::to_bits))
        .collect();
    bits.extend(pose.weights().iter().map(|weight| weight.to_bits()));
    bits
}

/// The same bits at a tick however the frames fell: a frame a tick, one every 7, or straight there.
#[test]
fn the_eyes_at_a_tick_are_the_same_however_the_frames_fell() {
    for kind in [LookAtKind::Bone, LookAtKind::Expression] {
        let character = Character::new(kind);
        let source = character.humanoid.source();
        let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
        let mut by_one = character.player(&graph, 7);
        let mut by_seven = character.player(&graph, 7);
        let mut moved = 0;
        let mut last = None;
        for tick in 0..=1400_u32 {
            by_one.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
            if tick % 7 != 0 {
                continue;
            }
            by_seven.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
            let mut straight = character.player(&graph, 7);
            straight.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
            let shown = eyes_shown(&character, &by_one);
            assert_eq!(shown, eyes_shown(&character, &by_seven), "{kind:?} by 7 at {tick}");
            assert_eq!(shown, eyes_shown(&character, &straight), "{kind:?} straight to {tick}");
            moved += usize::from(last.as_ref().is_some_and(|last| *last != shown));
            last = Some(shown);
        }
        // About a jump every 95 ticks, each seen at a few frames 7 apart.
        assert!(moved >= 14, "{kind:?}: the eyes moved at {moved} frames");
    }
}

/// Two characters' seeds are two gazes, and the gaze is the clock's: a frozen clock holds it.
#[test]
fn two_seeds_look_apart_and_a_frozen_clock_holds_the_gaze() {
    let character = Character::new(LookAtKind::Bone);
    let source = character.humanoid.source();
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let eyes = graph.eyes.as_ref().expect("eyes");
    let apart =
        (0..2000).filter(|&tick| eyes.gaze(f64::from(tick), 1) != eyes.gaze(f64::from(tick), 2)).count();
    assert!(apart > 1900, "the two gazes agree at {} ticks of 2000", 2000 - apart);
    let mut player = character.player(&graph, 1);
    // Mid-jump, found by looking.
    let tick = (1..2000)
        .map(f64::from)
        .find(|&tick| {
            eyes.gaze(tick, 1) != eyes.gaze(tick - 1.0, 1) && eyes.gaze(tick, 1) != eyes.gaze(tick + 1.0, 1)
        })
        .expect("a jump");
    player.evaluate(&idle(tick), source.as_slice(), 0);
    let held = eyes_shown(&character, &player);
    assert!(!player.evaluate(&idle(tick), source.as_slice(), 0), "a frozen clock evaluates nothing");
    assert_eq!(eyes_shown(&character, &player), held);
}

/// The gaze holds still between jumps for the document's hold, and wanders within its reach, the middle
/// likelier than the edge; a jump takes its length at the document's speed.
#[test]
fn the_gaze_holds_then_jumps_within_its_reach() {
    let character = Character::new(LookAtKind::Bone);
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let eyes = graph.eyes.as_ref().expect("eyes");
    let reach = REACH.map(f32::to_radians);
    let mut starts = Vec::new();
    let mut near_middle = 0;
    let mut last = eyes.gaze(0.0, 3);
    let mut moving = false;
    for tick in 1..20_000 {
        let gaze = eyes.gaze(f64::from(tick), 3);
        assert!(gaze[0].abs() <= reach[0] && gaze[1].abs() <= reach[1], "{gaze:?} at {tick}");
        let moves = gaze != last;
        if moves && !moving {
            starts.push(tick);
            let target = eyes.jump(f64::from(tick), 3).to;
            near_middle += usize::from(target[0].abs() < reach[0] * 0.5);
        }
        moving = moves;
        last = gaze;
    }
    assert!(starts.len() > 150, "{} jumps", starts.len());
    for pair in starts.windows(2) {
        // Within a tick either way of the document's hold, for where a start falls among ticks.
        let gap = pair[1] - pair[0];
        assert!((HOLD[0] - 1..=HOLD[1] + 1).contains(&gap), "{gap} in {starts:?}");
    }
    // A triangle's middle half holds three quarters of it.
    assert!(near_middle * 100 > starts.len() * 65, "{near_middle} of {} near the middle", starts.len());
    // A jump's length: its distance at the speed, at least a tick.
    let jump = eyes.jump(f64::from(starts[3]), 3);
    assert_eq!(jump.length, (eyes::distance(jump.from, jump.to) / SPEED.to_radians()).max(1.0));
}

/// The eyes turn as VRM's look-at says: a gaze to the character's left turns both eyes' fronts toward +X,
/// the left eye by its outer range and the right by its inner, here 10° at 90°; or weighs `lookLeft`.
#[test]
fn the_eyes_turn_through_the_rigs_look_at() {
    for kind in [LookAtKind::Bone, LookAtKind::Expression] {
        let character = Character::new(kind);
        let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
        let eyes = graph.eyes.as_ref().expect("eyes");
        let mut player = character.player(&graph, 1);
        let gaze = [20f32.to_radians(), -10f32.to_radians()];
        eyes.apply(gaze, 1.0, &mut player.pose, PoseMath::Legacy);
        let pose = &player.pose;
        if kind == LookAtKind::Bone {
            for bone in [HumanBone::LeftEye, HumanBone::RightEye] {
                // The humanoid's joints all rest unturned, so a local turn is a model turn.
                let front = pose.locals()[character.eye(bone)].rotation * Vec3::Z;
                let yaw = front.x.atan2(front.z).to_degrees();
                let pitch = front.y.asin().to_degrees();
                assert!(
                    (yaw - 20.0 / 9.0).abs() < 1e-3 && (pitch + 10.0 / 9.0).abs() < 1e-3,
                    "{bone:?}: {yaw}, {pitch}"
                );
            }
        } else {
            let weights = pose.weights();
            assert!((weights[LOOK_LEFT] - 20.0 / 30.0).abs() < 1e-6, "{weights:?}");
            assert!((weights[LOOK_DOWN] - 10.0 / 30.0).abs() < 1e-6, "{weights:?}");
            assert_eq!([weights[LOOK_RIGHT], weights[LOOK_UP]], [0.0, 0.0]);
        }
    }
}

/// A face whose expression blocks the look holds bone eyes at rest, and one that blends it scales their
/// turn; on a look-at of expressions the share is the face layer's to apply, so the graph leaves it be.
#[test]
fn the_faces_share_turns_bone_eyes_down() {
    let character = Character::new(LookAtKind::Bone);
    let source = character.humanoid.source();
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let resting = character.bind(&program(&text("", ""), Target::COOKED));
    let eyes = graph.eyes.as_ref().expect("eyes");
    let tick = (0..2000).map(f64::from).find(|&tick| eyes.gaze(tick, 1)[0].abs() > 0.2).expect("a wide gaze");
    let left = character.eye(HumanBone::LeftEye);
    let turn_at = |graph: &Arc<GraphBinding>, share: f32| {
        let mut player = character.player(graph, 1);
        player.set_eyes_share(share);
        player.evaluate(&idle(tick), source.as_slice(), 0);
        player.pose().locals()[left].rotation
    };
    let rest = turn_at(&resting, 1.0);
    assert_eq!(
        turn_at(&graph, 0.0).to_array().map(f32::to_bits),
        rest.to_array().map(f32::to_bits),
        "blocked"
    );
    let whole = (turn_at(&graph, 1.0) * rest.inverse()).to_axis_angle().1;
    let half = (turn_at(&graph, 0.5) * rest.inverse()).to_axis_angle().1;
    assert!(whole > 0.01 && (half - whole * 0.5).abs() < 1e-4, "{whole} then {half}");
    // A share set on a frozen clock shows at its next evaluation.
    let mut player = character.player(&graph, 1);
    player.evaluate(&idle(tick), source.as_slice(), 0);
    player.set_eyes_share(0.0);
    assert!(player.evaluate(&idle(tick), source.as_slice(), 0));
    assert_eq!(
        player.pose().locals()[left].rotation.to_array().map(f32::to_bits),
        rest.to_array().map(f32::to_bits)
    );
    // A look-at of expressions keeps the whole gaze: the face layer turns those morphs down itself.
    let expressions = Character::new(LookAtKind::Expression);
    let graph = expressions.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let mut player = expressions.player(&graph, 1);
    player.set_eyes_share(0.0);
    assert_eq!(player.eyes_share, 1.0);
}

/// A face blending its look override on a frozen clock moves only the share: each frame hands the same
/// output over again with the eyes turned by it, and evaluates none of the layers.
#[test]
fn a_share_alone_hands_over_without_evaluating() {
    let character = Character::new(LookAtKind::Bone);
    let source = character.humanoid.source();
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let eyes = graph.eyes.clone().expect("eyes");
    let tick = (0..2000).map(f64::from).find(|&tick| eyes.gaze(tick, 1)[0].abs() > 0.2).expect("a wide gaze");
    let left = character.eye(HumanBone::LeftEye);
    let mut player = character.player(&graph, 1);
    player.evaluate(&idle(tick - 1.0), source.as_slice(), 0);
    player.evaluate(&idle(tick), source.as_slice(), 0);
    let mut fresh = character.player(&graph, 1);
    let mut evaluated = 0;
    for frame in 0..5 {
        let share = 1.0 - frame as f32 / 4.0;
        player.reached = 0;
        player.set_eyes_share(share);
        assert!(player.evaluate(&idle(tick), source.as_slice(), 0) || frame == 0, "frame {frame} shows");
        evaluated += usize::from(player.reached & Reached::FROZEN != 0);
        // The same as a player that evaluated the frame whole with that share.
        fresh.set_eyes_share(share);
        fresh.evaluate(&idle(tick - 1.0), source.as_slice(), 0);
        fresh.evaluate(&idle(tick), source.as_slice(), 0);
        let rotation =
            |player: &GraphPlayer| player.pose().locals()[left].rotation.to_array().map(f32::to_bits);
        assert_eq!(rotation(&player), rotation(&fresh), "frame {frame}");
    }
    assert_eq!(evaluated, 0, "no frame evaluated the layers");
}

/// The fixed step never turns the eyes: its pose's eye bones are the layers' alone.
#[test]
fn the_fixed_step_never_turns_the_eyes() {
    let character = Character::new(LookAtKind::Bone);
    let source = character.humanoid.source();
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::Simulation));
    let resting = character.bind(&program(&text("", ""), Target::Simulation));
    let eyes = graph.eyes.as_ref().expect("eyes");
    let tick = (0..2000).map(f64::from).find(|&tick| eyes.gaze(tick, 1)[0].abs() > 0.2).expect("a wide gaze");
    let left = character.eye(HumanBone::LeftEye);
    let fixed = |graph: &Arc<GraphBinding>| {
        let mut player = character.player(graph, 1);
        player.layers.math = PoseMath::Pinned;
        player.inertia.set_math(PoseMath::Pinned);
        player.evaluate(&idle(tick), source.as_slice(), 0);
        player.pose().locals()[left].rotation.to_array().map(f32::to_bits)
    };
    assert_eq!(fixed(&graph), fixed(&resting));
}

/// Over a pose the fixed step published, where the player plays nothing, the eyes turn on the owner's
/// clock (`turn_published_eyes`): bones with their model transforms refreshed, gaze by gaze; a look-at of
/// expressions leaves the published morphs alone.
#[test]
fn the_eyes_turn_over_a_published_pose() {
    let character = Character::new(LookAtKind::Bone);
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let eyes = graph.eyes.clone().expect("eyes");
    let mut player = character.player(&graph, 1);
    player.finish();
    let rest = player.pose().locals().to_vec();
    let wide = (0..2000).map(f64::from).find(|&tick| eyes.gaze(tick, 1)[0] > 0.2).expect("a wide gaze");
    let other = (0..2000).map(f64::from).find(|&tick| eyes.gaze(tick, 1)[0] < -0.2).expect("the other way");
    assert_ne!(player.published_eyes(wide), player.published_eyes(other));
    let [left, right] = [HumanBone::LeftEye, HumanBone::RightEye].map(|bone| character.eye(bone));
    let turned = [wide, other].map(|tick| {
        player.pose_mut().locals_mut().copy_from_slice(&rest);
        player.finish();
        assert!(player.turn_published_eyes(tick));
        let pose = player.pose();
        for eye in [left, right] {
            assert_ne!(pose.locals()[eye].rotation, rest[eye].rotation, "{tick}: the eye turned");
            let parent = u16::try_from(pose.parent(eye).expect("an eye has a head")).expect("a joint");
            let model = crate::model::model_of(parent, &pose.locals()[eye], pose.model());
            assert_eq!(pose.model()[eye].to_cols_array(), model.to_cols_array(), "{tick}: its model follows");
        }
        pose.locals()[left].rotation
    });
    assert_ne!(turned[0], turned[1], "a gaze each");

    let character = Character::new(LookAtKind::Expression);
    let graph = character.bind(&program(&text(&eyes_of(0.0), ""), Target::COOKED));
    let mut player = character.player(&graph, 1);
    let weights = player.pose().weights().to_vec();
    assert!(!player.turn_published_eyes(wide), "the published morphs are the simulation's");
    assert_eq!(player.pose().weights(), weights.as_slice());
}

/// With a chance, a long jump brings a blink at its start, in the blink's own shape; with none, the blinks
/// are the blink's alone.
#[test]
fn a_long_jump_may_blink() {
    let character = Character::new(LookAtKind::Bone);
    let source = character.humanoid.source();
    // A blink of its own rarely: once in 5000 to 6000 ticks.
    let blink = r#"blink: Some((expression: "blink", every: (5000, 6000), close: 3, hold: 1, open: 7)),"#;
    let shut_starts = |chance: f32| {
        let graph = character.bind(&program(&text(&eyes_of(chance), blink), Target::COOKED));
        let eyes = graph.eyes.clone().expect("eyes");
        let mut player = character.player(&graph, 5);
        let (mut starts, mut with_jumps) = (0, 0);
        let mut last = 0.0;
        for tick in 0..4000 {
            player.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
            let shut = player.pose().weights()[BLINK];
            if last == 0.0 && shut > 0.0 {
                starts += 1;
                let jump = eyes.jump(f64::from(tick), 5);
                with_jumps += usize::from(f64::from(tick) - jump.start < 1.0);
            }
            last = shut;
        }
        (starts, with_jumps)
    };
    let (starts, with_jumps) = shut_starts(1.0);
    assert!(starts >= 10 && with_jumps + 1 >= starts, "{with_jumps} of {starts} blinks with a jump");
    assert!(shut_starts(0.0).0 <= 1, "the blink's own alone");
}

/// A graph's eyes need a look-at the rig can move.
#[test]
fn eyes_need_a_look_at() {
    let character = Character::new(LookAtKind::Bone);
    let mut rig = humanoid::humanoid_rig();
    rig.look_at = None;
    let rig = rig.to_cooked().expect("it cooks");
    let rig = CharacterRig::read(&rig).expect("a rig");
    let source = character.humanoid.source();
    let program = program(&text(&eyes_of(0.0), ""), Target::COOKED);
    let error =
        GraphBinding::from_program(&program, character.humanoid.skeleton(), Some(rig), 3, source.as_slice())
            .expect_err("no look-at");
    assert!(matches!(error, GraphBindError::NoLookAt), "{error}");
}

/// The close-up goldens' two ticks (`lookdev-face-eyes-held` and `-jumped` in `tests/golden/scenes.ron`) are
/// each inside a hold of the heroine's gaze, on her seed, with no blink showing (her own or a jump's) and a
/// jump between them that a close-up shows.
#[test]
fn the_close_ups_two_ticks_are_a_saccade_apart() {
    const TICKS: [f64; 2] = [8.0, 60.0];
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../content/lookdev/character/character.animgraph.ron");
    let graph = humanoid::read(&std::fs::read_to_string(path).expect("the heroine's graph"));
    let humanoid = Humanoid::new(graph.clips.iter().map(|clip| clip.id));
    let binding = humanoid.bind(&graph.compile_for(Target::COOKED).expect("it compiles"));
    let eyes = binding.eyes.as_ref().expect("her graph has eyes");
    let degrees = |tick: f64| eyes.gaze(tick, 1).map(f32::to_degrees);
    let blink = binding.blink.as_ref().expect("her graph blinks");
    let shut = |tick: f64| {
        let jumps = eyes.blinks(tick, 1).into_iter().flatten();
        jumps.map(|start| blink.shape((tick - start) as f32)).fold(blink.amount(tick, 1), f32::max)
    };
    let schedule: Vec<String> = (0..12)
        .map(|cell| {
            let jump = eyes.jump_of(cell, 1);
            format!("{:.1}+{:.1}: {:?}", jump.start, jump.length, jump.to.map(f32::to_degrees))
        })
        .collect();
    for tick in TICKS {
        assert!(
            degrees(tick - 3.0) == degrees(tick) && degrees(tick + 3.0) == degrees(tick),
            "{tick} isn't inside a hold: {schedule:?}"
        );
        let shown = [-3.0, 0.0, 3.0].map(|near| shut(tick + near));
        assert_eq!(shown, [0.0; 3], "{tick} shows a blink");
    }
    let [held, jumped] = TICKS.map(degrees);
    let apart = eyes::distance(held, jumped);
    assert!(apart > 30.0, "{held:?} to {jumped:?}: {schedule:?}");
}
