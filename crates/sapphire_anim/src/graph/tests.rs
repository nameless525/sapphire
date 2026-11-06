use std::sync::Arc;

use sapphire_assets::AssetId;
use sapphire_assets::clip::{
    ArchivedClip, Clip, DEFAULT_RATE, PackedRotation, RotationTrack, WeightRange, WeightTrack,
};
use sapphire_assets::graph::AnimGraph;
use sapphire_assets::rig::{
    CharacterRig, Expression, ExpressionOverride, ExpressionPreset, HumanBone, HumanoidJoint,
    MorphTargetWeight,
};
use sapphire_assets::skeleton::{ArchivedSkeleton, Joint, JointTransform, Skeleton};
use sapphire_core::Name;
use sapphire_math::{Quat, Transform, Vec3};
use sapphire_serialize::cooked::AlignedVec;

use super::*;
use crate::{overlay, sample};

/// Five that do things, then a leaf for each bone a humanoid rig must have.
const JOINTS: usize = 5 + HumanBone::REQUIRED.len() - 3;
const HIPS: u16 = 0;
const SPINE: u16 = 1;
const ARM: u16 = 2;
const HAND: u16 = 3;

/// Which morph target the rig's `blink` closes.
const BLINK_TARGET: usize = 1;

fn joints() -> Vec<Joint> {
    let joint = |name: &str, parent, translation| Joint {
        name: name.to_owned(),
        parent,
        rest: JointTransform { translation, ..JointTransform::IDENTITY },
    };
    let mut joints = vec![
        joint("hips", None, [0.0, 1.0, 0.0]),
        joint("spine", Some(HIPS), [0.0, 0.3, 0.0]),
        joint("arm", Some(SPINE), [0.2, 0.2, 0.0]),
        joint("hand", Some(ARM), [0.5, 0.0, 0.0]),
        // Beside the hips, and no clip moves it.
        joint("still", Some(HIPS), [0.0, -0.5, 0.2]),
    ];
    joints.resize_with(JOINTS, || joint("leaf", Some(HIPS), [0.0, -0.1, 0.0]));
    joints
}

/// Every bone a rig must have: the three the clips move at their joints, and the rest on the leaves.
fn humanoid() -> Vec<HumanoidJoint> {
    let mut leaf = 5..;
    let mut humanoid: Vec<HumanoidJoint> = HumanBone::REQUIRED
        .iter()
        .map(|&bone| {
            let joint = match bone {
                HumanBone::Hips => HIPS,
                HumanBone::Spine => SPINE,
                HumanBone::RightUpperArm => ARM,
                _ => leaf.next().expect("an endless range"),
            };
            HumanoidJoint { bone, joint }
        })
        .collect();
    humanoid.sort_by_key(|entry| entry.bone);
    humanoid
}

fn rig() -> CharacterRig {
    CharacterRig {
        joint_count: JOINTS as u32,
        morph_target_count: 3,
        humanoid: humanoid(),
        expressions: vec![Expression {
            name: "blink".to_owned(),
            preset: Some(ExpressionPreset::Blink),
            binary: false,
            override_blink: ExpressionOverride::None,
            override_look_at: ExpressionOverride::None,
            override_mouth: ExpressionOverride::None,
            morph_targets: vec![MorphTargetWeight { target: BLINK_TARGET as u32, weight: 1.0 }],
            material_colors: Vec::new(),
            texture_transforms: Vec::new(),
        }],
        ..CharacterRig::default()
    }
}

/// `ticks` long, turning `joint` about `axis` from `from` to `to` radians at a steady rate, with
/// nothing else keyed.
fn turning(ticks: u16, joint: u16, axis: Vec3, from: f32, to: f32) -> Clip {
    let key = |angle: f32| PackedRotation::encode(Quat::from_axis_angle(axis, angle).to_array());
    Clip {
        name: "turning".to_owned(),
        skeleton: Some(AssetId::from_bytes([9; 16])),
        joint_count: JOINTS as u32,
        morph_target_count: 3,
        rate: DEFAULT_RATE,
        ticks,
        rotations: vec![RotationTrack { joint, count: 2, first: 0 }],
        rotation_ticks: vec![0, ticks],
        rotation_keys: vec![key(from), key(to)],
        ..Clip::default()
    }
}

/// `clip`, with a morph target going from 0 to 1 over its length.
fn with_morph(mut clip: Clip, target: u16) -> Clip {
    let range = WeightRange::enclosing([0.0, 1.0]);
    clip.weights = vec![WeightTrack { target, count: 2, first: 0, range }];
    clip.weight_ticks = vec![0, clip.ticks];
    clip.weight_keys = vec![range.encode(0.0), range.encode(1.0)];
    clip
}

/// A fixture's cooked assets, kept alive for what reads them in place.
struct Fixture {
    skeleton: AlignedVec,
    rig: AlignedVec,
    clips: Vec<(&'static str, AssetId, AlignedVec)>,
}

fn id(name: &str) -> AssetId {
    let mut bytes = [7; 16];
    for (byte, source) in bytes.iter_mut().zip(name.bytes()) {
        *byte = source;
    }
    AssetId::from_bytes(bytes)
}

/// The clips every test here has: a walk and a run cycle on the hips, a jump's loop and landing on
/// the spine, two moves on the arm and a slow additive on the spine.
fn fixture() -> Fixture {
    let clip = |name: &'static str, clip: Clip| (name, id(name), clip.to_cooked().expect("it cooks"));
    Fixture {
        skeleton: Skeleton::new(&joints()).to_cooked().expect("it cooks"),
        rig: rig().to_cooked().expect("it cooks"),
        clips: vec![
            clip("idle", turning(120, SPINE, Vec3::X, 0.0, 0.1)),
            clip("walk", turning(60, HIPS, Vec3::Y, -0.3, 0.3)),
            clip("run", turning(30, HIPS, Vec3::Z, -0.5, 0.5)),
            clip("air", turning(30, SPINE, Vec3::Z, 0.6, 0.6)),
            clip("land", turning(10, SPINE, Vec3::X, 0.3, 0.0)),
            clip("jab", with_morph(turning(60, ARM, Vec3::Z, 0.0, 1.2), 2)),
            clip("hook", turning(60, ARM, Vec3::Y, 1.5, -1.0)),
            clip("wave", turning(60, HAND, Vec3::Z, 0.0, 1.0)),
            clip("breath", turning(60, SPINE, Vec3::Y, 0.0, 0.2)),
            clip("flinch", turning(20, SPINE, Vec3::Z, 0.0, 0.5)),
        ],
    }
}

impl Fixture {
    fn skeleton(&self) -> &ArchivedSkeleton {
        Skeleton::read(&self.skeleton).expect("a valid skeleton")
    }

    fn clip(&self, name: &str) -> &ArchivedClip {
        let (_, _, bytes) = self.clips.iter().find(|(clip, _, _)| *clip == name).expect("a known clip");
        Clip::read(bytes).expect("a valid clip")
    }

    fn source(&self) -> Vec<(AssetId, &ArchivedClip)> {
        self.clips.iter().map(|(name, id, _)| (*id, self.clip(name))).collect()
    }

    fn graph(&self, text: &str) -> AnimGraph {
        let mut graph = AnimGraph::from_document(text).expect("a good document");
        graph.resolve(|clip| id(&clip.name));
        graph
    }

    fn bind(&self, graph: &AnimGraph) -> Arc<GraphBinding> {
        let rig = CharacterRig::read(&self.rig).expect("a valid rig");
        let source = self.source();
        Arc::new(
            GraphBinding::new(graph, self.skeleton(), Some(rig), 3, source.as_slice()).expect("it binds"),
        )
    }

    fn pose(&self) -> CharacterPose {
        CharacterPose::new(self.skeleton(), &[], &[], vec![0.0, 0.0, 0.25]).with_morph_names(vec![
            "smile".into(),
            "blink".into(),
            "rest".into(),
        ])
    }

    fn player(&self, text: &str) -> GraphPlayer {
        GraphPlayer::new(self.bind(&self.graph(text)), self.pose(), 11)
    }
}

/// `body` in a document, with every clip the fixture has under its own name.
fn document(body: &str) -> String {
    let clips: Vec<String> =
        fixture().clips.iter().map(|(name, id, _)| format!(r#"(name: "{name}", source: "{id}")"#)).collect();
    format!(
        r#"(type: "sapphire::assets::AnimGraph", version: 1, data: (clips: [{}], {body}))"#,
        clips.join(", ")
    )
}

/// Standing still in one clip, with moves: the plainest thing a cancel can leave.
fn plain() -> String {
    document(
        r#"
        locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "idle", looping: true))]),
        combat: (decay: 8, exit: 12, moves: [
            (move: "jab", clip: "jab"),
            (move: "hook", clip: "hook", decay: 5),
            (move: "wave", clip: "wave"),
        ]),
        "#,
    )
}

const JAB: Name = Name::new("jab");
const HOOK: Name = Name::new("hook");
const WAVE: Name = Name::new("wave");

/// A frame at `ticks` with `name`'s playing `serial` at `position`.
fn in_move(ticks: f64, name: Name, serial: u32, position: f64) -> GraphInput {
    GraphInput { ticks, combat: Some(CombatInput { name, serial, position }), ..GraphInput::default() }
}

fn idle(ticks: f64) -> GraphInput {
    GraphInput { ticks, ..GraphInput::default() }
}

fn bits(pose: &[Transform]) -> Vec<u32> {
    pose.iter()
        .flat_map(|joint| {
            joint
                .translation
                .to_array()
                .into_iter()
                .chain(joint.rotation.to_array())
                .chain(joint.scale.to_array())
                .map(f32::to_bits)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn hand_at(player: &mut GraphPlayer) -> Vec3 {
    player.finish();
    Vec3::from(player.pose().model()[usize::from(HAND)].translation)
}

/// The rotation, in the parent's frame, of `joint`.
fn turn_of(player: &GraphPlayer, joint: u16) -> Quat {
    player.pose().locals()[usize::from(joint)].rotation
}

/// What a clip source has to keep: the graph playing's until the next state's start, and the waiting one's
/// from then, with nothing of the graph that's gone.
#[test]
fn a_player_says_which_clips_it_may_still_read() {
    let fixture = fixture();
    let named =
        |text: &str| -> Vec<AssetId> { fixture.graph(text).clips.iter().map(|clip| clip.id).collect() };
    let mut player = fixture.player(&plain());
    assert_eq!(player.clips_in_use().collect::<Vec<_>>(), named(&plain()));

    player.replace(fixture.bind(&fixture.graph(&jumping())));
    let both = [named(&plain()), named(&jumping())].concat();
    assert_eq!(player.clips_in_use().collect::<Vec<_>>(), both);
}

#[test]
fn a_frozen_clock_evaluates_nothing_and_holds_every_bit() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    assert!(player.evaluate(&in_move(10.0, JAB, 1, 3.0), source.as_slice(), 0));
    player.finish();
    let held = bits(player.pose().locals());
    let weights = player.pose().weights().to_vec();
    for _ in 0..10 {
        assert!(!player.evaluate(&in_move(10.0, JAB, 1, 3.0), source.as_slice(), 0), "nothing to do");
        assert_eq!(bits(player.pose().locals()), held);
        assert_eq!(player.pose().weights(), weights);
    }
}

/// The key covers every input: N2's skip was on position alone.
#[test]
fn a_parameter_changed_on_a_frozen_clock_still_shows() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&document(
        r#"locomotion: (start: "ground", smoothing: 8.0, states: [(name: "ground", plays: Blend((samples: [
            (clip: "idle", at: (0.0, 0.0), sync: false),
            (clip: "run", at: (0.0, 5.0)),
        ])))]),"#,
    ));
    let still = idle(10.0);
    assert!(player.evaluate(&still, source.as_slice(), 0));
    let before = player.pose().locals()[usize::from(HIPS)];
    let running =
        GraphInput { locomotion: LocomotionInput { speed: 5.0, ..LocomotionInput::default() }, ..still };
    assert!(player.evaluate(&running, source.as_slice(), 0), "the input changed");
    let after = player.pose().locals()[usize::from(HIPS)];
    assert_ne!(before, after, "the blend moved, with no time passing");
    // The whole of it: there's no time to smooth over, so it's exactly the run.
    let mut expected = fixture.pose().rest().to_vec();
    overlay(fixture.clip("run"), 0.0, &mut expected);
    assert_eq!(after, expected[usize::from(HIPS)]);
}

#[test]
fn a_new_version_of_a_clip_on_a_frozen_clock_still_shows() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    assert!(player.evaluate(&idle(4.0), source.as_slice(), 0));
    assert!(!player.evaluate(&idle(4.0), source.as_slice(), 0));
    assert!(player.evaluate(&idle(4.0), source.as_slice(), 1), "the clips' version moved");
}

/// A move's pose is its clip at the move's elapsed, whole ticks or not: the very number its tables are
/// read at. That's how a pose and a hit volume agree on the same tick.
#[test]
fn a_moves_position_is_its_elapsed_time_tick_for_tick() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    let rest = fixture.pose().rest().to_vec();
    let mut expected = rest.clone();
    for (frame, elapsed) in [0.0, 0.25, 1.0, 2.5, 7.0, 30.0, 59.75, 60.0, 90.0].into_iter().enumerate() {
        let input = in_move(100.0 + frame as f64, JAB, 1, elapsed);
        player.evaluate(&input, source.as_slice(), 0);
        sample(fixture.clip("jab"), elapsed as f32, &rest, &mut expected);
        assert_eq!(bits(player.target()), bits(&expected), "at {elapsed}");
    }
    // A clock that's stopped or slowed is the same read: only the position counts.
    let input = in_move(200.0, JAB, 1, 13.5);
    player.evaluate(&input, source.as_slice(), 0);
    sample(fixture.clip("jab"), 13.5, &rest, &mut expected);
    assert_eq!(bits(player.target()), bits(&expected));
}

/// A frame can draw the instant before a move the simulation has started: it holds the first tick.
#[test]
fn a_position_before_the_move_reads_its_first_tick() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    player.evaluate(&in_move(5.0, JAB, 1, -0.6), source.as_slice(), 0);
    let rest = fixture.pose().rest().to_vec();
    let mut expected = rest.clone();
    sample(fixture.clip("jab"), 0.0, &rest, &mut expected);
    assert_eq!(bits(player.target()), bits(&expected));
}

/// Frames `h` ticks apart through a cancel from `jab` into `hook` at the frame at tick 20, so the first
/// frame to see it, a frame later, is `h` into the new move.
fn cancel_run(fixture: &Fixture, h: f64) -> (Vec<Vec3>, GraphPlayer, usize) {
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    let mut positions = Vec::new();
    let frames = (40.0 / h) as usize;
    let cancel = (20.0 / h) as usize;
    for frame in 0..frames {
        let t = frame as f64 * h;
        let input = if frame <= cancel { in_move(t, JAB, 1, t) } else { in_move(t, HOOK, 2, t - 20.0) };
        player.evaluate(&input, source.as_slice(), 0);
        positions.push(hand_at(&mut player));
    }
    (positions, player, cancel + 1)
}

/// Across a cancel the hand doesn't jump: the step across it goes to nothing with the frame's time.
#[test]
fn a_cancel_is_continuous_in_pose() {
    let fixture = fixture();
    for h in [1.0 / 64.0, 0.125] {
        let (positions, _, cancel) = cancel_run(&fixture, h);
        let across = positions[cancel].distance(positions[cancel - 1]);
        // At the steep end of a 5-tick decay of a turn of 1.6 rad, about what the old motion and the
        // decay's acceleration make in that time, far from the half metre the poses are apart.
        assert!(f64::from(across) < 0.3 * h, "at h {h}: {across}");
    }
    assert!(hand_apart(&fixture) > 0.4, "the poses it crosses are far apart: {}", hand_apart(&fixture));
}

/// How far the hand is from where the new clip's first tick has it, at the instant of the cancel.
fn hand_apart(fixture: &Fixture) -> f32 {
    let source = fixture.source();
    let mut old = fixture.player(&plain());
    old.evaluate(&in_move(20.0, JAB, 1, 20.0), source.as_slice(), 0);
    let mut new = fixture.player(&plain());
    new.evaluate(&in_move(20.0, HOOK, 2, 0.0), source.as_slice(), 0);
    hand_at(&mut old).distance(hand_at(&mut new))
}

/// Across a cancel the velocity has no jump: the difference between the steps either side of it, as a
/// velocity, is the decay's acceleration times the step, and goes to nothing with the step.
#[test]
fn a_cancel_is_continuous_in_velocity() {
    let fixture = fixture();
    let jump = |h: f64| {
        let (positions, _, cancel) = cancel_run(&fixture, h);
        let before = positions[cancel - 1] - positions[cancel - 2];
        let across = positions[cancel] - positions[cancel - 1];
        (across - before).length() as f64 / h
    };
    let (coarse, fine) = (jump(0.25), jump(1.0 / 64.0));
    assert!(fine < coarse * 0.2, "{coarse} at 1/4, {fine} at 1/64");
    assert!(fine < 0.02, "{fine}");
}

/// The new pose is at full weight from the first frame, and the decay ends on its exact bits. No fade.
#[test]
fn a_cancel_plays_the_new_clip_at_once_and_ends_on_its_exact_bits() {
    let fixture = fixture();
    let source = fixture.source();
    let rest = fixture.pose().rest().to_vec();
    let mut player = fixture.player(&plain());
    let mut expected = rest.clone();
    for tick in 0..20 {
        player.evaluate(&in_move(f64::from(tick), JAB, 1, f64::from(tick)), source.as_slice(), 0);
    }
    for tick in 20_u32..40 {
        let position = f64::from(tick - 20);
        player.evaluate(&in_move(f64::from(tick), HOOK, 2, position), source.as_slice(), 0);
        sample(fixture.clip("hook"), position as f32, &rest, &mut expected);
        // The layers' target is the new clip from the first frame.
        assert_eq!(bits(player.target()), bits(&expected), "at tick {tick}");
        // The decay counts from the frame before the cancel, tick 19, and lasts 5.
        if tick >= 24 {
            assert_eq!(bits(player.pose().locals()), bits(&expected), "at tick {tick}");
            assert!(!player.is_decaying());
        } else {
            assert_ne!(bits(player.pose().locals()), bits(&expected), "at tick {tick}");
            assert!(player.is_decaying());
        }
    }
}

/// How fast a cancel reads, against the fraction of the pose difference closed, with both clips
/// standing still so nothing else moves the number: `decay: 5` has moved 38% by the first frame
/// after, 79% by the second, 96% by the third, and goes on from there to the new pose.
#[test]
fn the_first_frames_after_a_cancel_already_read_as_the_new_move() {
    let fixture = fixture();
    let still = |name: &'static str, angle: f32| {
        (name, id(name), turning(30, ARM, Vec3::Z, angle, angle).to_cooked().expect("it cooks"))
    };
    let fixture = Fixture {
        clips: {
            let mut clips = fixture.clips;
            clips.retain(|(name, _, _)| !matches!(*name, "jab" | "hook"));
            clips.extend([still("jab", 0.0), still("hook", 1.0)]);
            clips
        },
        ..fixture
    };
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    for tick in 0..10 {
        player.evaluate(&in_move(f64::from(tick), JAB, 1, 0.0), source.as_slice(), 0);
    }
    let closed = |player: &GraphPlayer| {
        let turn = turn_of(player, ARM);
        let (axis, angle) = turn.to_axis_angle();
        axis.z * angle
    };
    let mut frames = Vec::new();
    for tick in 10_u32..16 {
        player.evaluate(&in_move(f64::from(tick), HOOK, 2, 0.0), source.as_slice(), 0);
        frames.push(closed(&player));
    }
    for (frame, (got, want)) in frames.iter().zip([0.38, 0.79, 0.96, 0.995, 1.0, 1.0]).enumerate() {
        assert!((got - want).abs() < 0.01, "frame {frame}: {got} against {want}, of {frames:?}");
    }
}

/// A cancel of a cancel, inside the first's decay: no jump in pose or velocity, from what was on screen.
#[test]
fn a_second_transition_inside_the_first_stays_continuous() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    let h = 1.0 / 64.0;
    let mut positions = Vec::new();
    let frames = (30.0 / h) as usize;
    let (first, second) = ((10.0 / h) as usize, (12.0 / h) as usize);
    for frame in 0..frames {
        let t = frame as f64 * h;
        let input = match frame {
            frame if frame < first => in_move(t, JAB, 1, t),
            frame if frame < second => in_move(t, HOOK, 2, t - 10.0),
            _ => in_move(t, WAVE, 3, t - 12.0),
        };
        player.evaluate(&input, source.as_slice(), 0);
        positions.push(hand_at(&mut player));
    }
    assert_eq!(player.stats().transitions, 2);
    let step = |frame: usize| positions[frame + 1] - positions[frame];
    for frame in [second - 2, second - 1, second] {
        let change = (step(frame) - step(frame - 1)).length() as f64 / h;
        assert!(change < 0.02, "frame {frame} of a second cancel at {second}: {change}");
    }
    // And it still ends exactly on the last clip.
    let rest = fixture.pose().rest().to_vec();
    let mut expected = rest.clone();
    sample(fixture.clip("wave"), (frames - 1) as f32 * h as f32 - 12.0, &rest, &mut expected);
    assert_eq!(bits(player.pose().locals()), bits(&expected));
}

/// Hitstop: a move forced under a frozen clock (a hit's reaction, the one start a frozen clock allows)
/// changes nothing until the clock moves, then decays as if the freeze hadn't been.
#[test]
fn a_transition_during_hitstop_holds_then_lands() {
    let fixture = fixture();
    let source = fixture.source();
    // The run with a freeze at tick 20, and the reaction forced 5 frames into it.
    let mut frozen = fixture.player(&plain());
    let mut free = fixture.player(&plain());
    for tick in 0..=20 {
        let input = in_move(f64::from(tick), JAB, 1, f64::from(tick));
        frozen.evaluate(&input, source.as_slice(), 0);
        free.evaluate(&input, source.as_slice(), 0);
    }
    frozen.finish();
    let held = bits(frozen.pose().locals());
    // Five frames of the same input.
    for _ in 0..5 {
        assert!(!frozen.evaluate(&in_move(20.0, JAB, 1, 20.0), source.as_slice(), 0));
    }
    // The forced move lands while frozen, and draws nothing new.
    assert!(!frozen.evaluate(&in_move(20.0, HOOK, 2, 0.0), source.as_slice(), 0), "the pose holds");
    assert_eq!(bits(frozen.pose().locals()), held, "bit for bit");
    assert!(!frozen.evaluate(&in_move(20.0, HOOK, 2, 0.0), source.as_slice(), 0));
    assert_eq!(frozen.stats().transitions, 1);
    // The same transition with no freeze: the same frame is drawn on the same clock.
    assert!(!free.evaluate(&in_move(20.0, HOOK, 2, 0.0), source.as_slice(), 0));
    // When the clock moves, it goes on from the instant of the transition.
    for tick in 21..30 {
        let input = in_move(f64::from(tick), HOOK, 2, f64::from(tick) - 20.0);
        assert!(frozen.evaluate(&input, source.as_slice(), 0));
        free.evaluate(&input, source.as_slice(), 0);
        assert_eq!(bits(frozen.pose().locals()), bits(free.pose().locals()), "tick {tick}");
    }
    // Ending on the clip.
    let rest = fixture.pose().rest().to_vec();
    let mut expected = rest.clone();
    sample(fixture.clip("hook"), 9.0, &rest, &mut expected);
    assert_eq!(bits(frozen.pose().locals()), bits(&expected));
}

/// A move whose clock is stopped mid-decay holds what's left of the decay, and resumes it.
#[test]
fn a_frozen_clock_holds_a_decay_where_it_is() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    for tick in 0..=10 {
        player.evaluate(&in_move(f64::from(tick), JAB, 1, f64::from(tick)), source.as_slice(), 0);
    }
    player.evaluate(&in_move(11.0, HOOK, 2, 1.0), source.as_slice(), 0);
    assert!(player.is_decaying());
    let held = bits(player.pose().locals());
    for _ in 0..4 {
        assert!(!player.evaluate(&in_move(11.0, HOOK, 2, 1.0), source.as_slice(), 0));
    }
    assert_eq!(bits(player.pose().locals()), held);
    assert!(player.evaluate(&in_move(12.0, HOOK, 2, 2.0), source.as_slice(), 0));
    assert_ne!(bits(player.pose().locals()), held);
}

/// The same move again is a new playing, and a transition, as the serial says.
#[test]
fn a_serial_change_is_a_transition_even_to_the_same_move() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    for tick in 0..=30 {
        player.evaluate(&in_move(f64::from(tick), JAB, 1, f64::from(tick)), source.as_slice(), 0);
    }
    assert_eq!(player.stats().transitions, 0, "a first frame stands alone");
    player.evaluate(&in_move(31.0, JAB, 2, 1.0), source.as_slice(), 0);
    assert_eq!(player.stats().transitions, 1);
    assert!(player.is_decaying());
}

#[test]
fn a_move_ending_decays_back_to_locomotion() {
    let fixture = fixture();
    let source = fixture.source();
    let rest = fixture.pose().rest().to_vec();
    let mut player = fixture.player(&plain());
    for tick in 0..=30 {
        player.evaluate(&in_move(f64::from(tick), JAB, 1, f64::from(tick)), source.as_slice(), 0);
    }
    let before = hand_at(&mut player);
    player.evaluate(&idle(31.0), source.as_slice(), 0);
    let after = hand_at(&mut player);
    assert!(before.distance(after) < 0.1, "{before} to {after}");
    // `exit: 12`, from the last frame drawn before it ended.
    for tick in 32..50 {
        player.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
    }
    let mut expected = rest.clone();
    sample(fixture.clip("idle"), 49.0, &rest, &mut expected);
    assert_eq!(bits(player.pose().locals()), bits(&expected));
}

/// A move nothing in the table plays: locomotion goes on, and it's counted, not fatal.
#[test]
fn a_move_with_no_clip_leaves_locomotion_playing() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    player.evaluate(&idle(0.0), source.as_slice(), 0);
    player.evaluate(&in_move(1.0, Name::new("taunt"), 1, 0.0), source.as_slice(), 0);
    assert_eq!(player.stats().unmapped_moves, 1);
    assert_eq!(player.stats().transitions, 0);
}

/// A frame that starts over plays no clip for such a move either, and counts as any other frame does:
/// the first a player draws, and a cut.
#[test]
fn a_move_with_no_clip_counts_on_a_frame_that_starts_over() {
    let fixture = fixture();
    let source = fixture.source();
    let taunt = Name::new("taunt");
    let mut player = fixture.player(&plain());
    player.evaluate(&in_move(0.0, taunt, 1, 0.0), source.as_slice(), 0);
    assert_eq!(player.stats().unmapped_moves, 1, "the first frame");
    let cut = GraphInput { cut: true, ..in_move(1.0, taunt, 1, 1.0) };
    player.evaluate(&cut, source.as_slice(), 0);
    assert_eq!(player.stats().unmapped_moves, 2, "a cut");
    let mapped = GraphInput { cut: true, ..in_move(2.0, JAB, 2, 0.0) };
    player.evaluate(&mapped, source.as_slice(), 0);
    assert_eq!(player.stats().unmapped_moves, 2, "a move with a clip");
}

/// The combat layer's morph keys come through, and decay across a cancel as joints do.
#[test]
fn a_clips_morph_weights_play_and_decay_too() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    player.evaluate(&idle(0.0), source.as_slice(), 0);
    assert_eq!(player.pose().weights()[2], 0.25, "the rest weight, with no track in the idle");
    player.evaluate(&in_move(30.0, JAB, 1, 30.0), source.as_slice(), 0);
    assert!((player.pose().weights()[2] - 0.5).abs() < 0.01, "{:?}", player.pose().weights());
    // Across a cancel to a clip without the track: from what was there to its rest, with no jump.
    player.evaluate(&in_move(30.25, HOOK, 2, 0.25), source.as_slice(), 0);
    assert!((player.pose().weights()[2] - 0.5).abs() < 0.02, "{:?}", player.pose().weights());
    player.evaluate(&in_move(40.0, HOOK, 2, 10.0), source.as_slice(), 0);
    assert_eq!(player.pose().weights()[2], 0.25);
}

fn blended() -> String {
    document(
        r#"locomotion: (start: "ground", smoothing: 0.0, states: [
            (name: "ground", plays: Blend((rate: (0.5, 2.0), samples: [
                (clip: "idle", at: (0.0, 0.0), sync: false),
                (clip: "walk", at: (0.0, 1.0), phase: 0.5),
                (clip: "run", at: (0.0, 5.0)),
            ]))),
        ]),"#,
    )
}

fn moving(ticks: f64, speed: f32) -> GraphInput {
    GraphInput {
        ticks,
        locomotion: LocomotionInput { speed, ..LocomotionInput::default() },
        ..GraphInput::default()
    }
}

/// At a sample's own velocity the pose is exactly that clip, at the phase its own length gives.
#[test]
fn a_blend_space_is_exact_at_a_sample_and_keeps_its_clips_in_phase() {
    let fixture = fixture();
    let source = fixture.source();
    let rest = fixture.pose().rest().to_vec();
    let mut player = fixture.player(&blended());
    let mut expected = rest.clone();
    // The first frame starts the phase, and each tick moves it a clip's length's worth.
    for tick in 0_u32..100 {
        player.evaluate(&moving(f64::from(tick), 5.0), source.as_slice(), 0);
        // `run` is 30 ticks long, so its position is the tick, wrapped. This test's clips aren't
        // closed loops, so where a wrap falls is rounding's choice.
        if tick % 30 == 0 {
            continue;
        }
        sample(fixture.clip("run"), (tick % 30) as f32, &rest, &mut expected);
        for (got, want) in player.target().iter().zip(&expected) {
            assert!(got.rotation.abs_diff_eq(want.rotation, 2e-3), "tick {tick}");
        }
    }
    // `walk`'s phase is half a cycle on from the shared phase.
    let mut player = fixture.player(&blended());
    for tick in 0_u32..100 {
        player.evaluate(&moving(f64::from(tick), 1.0), source.as_slice(), 0);
        if (tick + 30) % 60 == 0 {
            continue;
        }
        let position = ((tick as f32 / 60.0 + 0.5) % 1.0) * 60.0;
        sample(fixture.clip("walk"), position, &rest, &mut expected);
        for (got, want) in player.target().iter().zip(&expected) {
            assert!(got.rotation.abs_diff_eq(want.rotation, 2e-3), "tick {tick}");
        }
    }
}

/// Playback scales so the feet match the speed: past the run's own speed it plays faster, up to the
/// document's limit, and a slow walk plays slower.
#[test]
fn playback_scales_to_match_the_speed_within_its_limits() {
    let fixture = fixture();
    let source = fixture.source();
    let cycles_after = |speed: f32| {
        let mut player = fixture.player(&blended());
        player.evaluate(&moving(0.0, speed), source.as_slice(), 0);
        player.evaluate(&moving(30.0, speed), source.as_slice(), 0);
        player.layers.phase
    };
    // 7.5 m/s against the run's 5: half again as fast: 1.5 × 30 / 30 cycles, wrapped to a half.
    assert!((cycles_after(7.5) - 0.5).abs() < 1e-3, "{}", cycles_after(7.5));
    // 50 m/s: the limit is twice as fast: 2 cycles, which wrap to nothing.
    assert!(cycles_after(50.0).abs() < 1e-3 || (cycles_after(50.0) - 1.0).abs() < 1e-3);
    // A slow walk: half speed (half as far along its 60-tick cycle, which the idle shares none of).
    assert!((cycles_after(0.5) - 0.25).abs() < 1e-3, "{}", cycles_after(0.5));
    // Standing still: the phase doesn't move.
    assert_eq!(cycles_after(0.0), 0.0);
}

#[test]
fn smoothing_follows_the_velocity_on_the_owners_clock() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&document(
        r#"locomotion: (start: "ground", smoothing: 4.0, states: [(name: "ground", plays: Blend((samples: [
            (clip: "idle", at: (0.0, 0.0), sync: false),
            (clip: "run", at: (0.0, 5.0)),
        ])))]),"#,
    ));
    player.evaluate(&moving(0.0, 0.0), source.as_slice(), 0);
    player.evaluate(&moving(4.0, 5.0), source.as_slice(), 0);
    // A critically damped spring from rest closes half the gap in one half-life.
    assert!((player.layers.velocity.y - 2.5).abs() < 1e-3, "{}", player.layers.velocity.y);
    for tick in 5..200 {
        player.evaluate(&moving(f64::from(tick), 5.0), source.as_slice(), 0);
    }
    assert!((player.layers.velocity.y - 5.0).abs() < 1e-3);
}

/// A state machine, with `more` of the document after it.
fn jumping_with(more: &str) -> String {
    let machine = r#"locomotion: (start: "ground", smoothing: 0.0,
            states: [
                (name: "ground", plays: Clip(clip: "idle", looping: true)),
                (name: "air", plays: Clip(clip: "air", looping: true)),
                (name: "land", plays: Clip(clip: "land")),
                (name: "run", plays: Clip(clip: "run", looping: true)),
            ],
            transitions: [
                (from: "ground", to: "air", when: Airborne, decay: 4),
                (from: "ground", to: "run", when: Moving(1.0), decay: 4),
                (from: "run", to: "ground", when: Stopped(1.0), decay: 4),
                (from: "air", to: "land", when: Landed, decay: 2),
                (from: "air", to: "ground", when: Grounded, decay: 6),
                (from: "land", to: "run", when: Moving(1.0), decay: 4),
                (from: "land", to: "ground", when: Finished, decay: 6),
            ],
        ),"#;
    document(&format!("{machine}{more}"))
}

fn jumping() -> String {
    jumping_with("")
}

/// The state machine takes its conditions in order, one change a frame, each inertialized.
#[test]
fn the_state_machine_moves_on_its_conditions() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&jumping());
    let input = |ticks: f64, speed: f32, grounded: bool, landed: bool| GraphInput {
        ticks,
        locomotion: LocomotionInput { speed, direction: 0.0, grounded, landed },
        ..GraphInput::default()
    };
    let (ground, air, land, run) =
        (Name::new("ground"), Name::new("air"), Name::new("land"), Name::new("run"));
    player.evaluate(&input(0.0, 0.0, true, false), source.as_slice(), 0);
    assert_eq!(player.state(), ground);
    player.evaluate(&input(1.0, 0.0, false, false), source.as_slice(), 0);
    assert_eq!(player.state(), air);
    assert_eq!(player.stats().transitions, 1);
    player.evaluate(&input(2.0, 0.0, false, false), source.as_slice(), 0);
    assert_eq!(player.state(), air, "still up");
    player.evaluate(&input(3.0, 0.0, true, true), source.as_slice(), 0);
    assert_eq!(player.state(), land, "`Landed` comes first");
    // It plays to its end, 10 ticks from the frame before the landing.
    player.evaluate(&input(11.0, 0.0, true, false), source.as_slice(), 0);
    assert_eq!(player.state(), land);
    player.evaluate(&input(12.0, 0.0, true, false), source.as_slice(), 0);
    assert_eq!(player.state(), ground);
    player.evaluate(&input(13.0, 3.0, true, false), source.as_slice(), 0);
    assert_eq!(player.state(), run);
    player.evaluate(&input(14.0, 0.0, true, false), source.as_slice(), 0);
    assert_eq!(player.state(), ground);
    assert_eq!(player.stats().transitions, 5);
}

/// A move hides locomotion, and locomotion keeps track underneath it, so its end lands in the right
/// state with no extra transition while it's hidden.
#[test]
fn locomotion_keeps_track_under_a_move() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player =
        fixture.player(&jumping_with(r#"combat: (decay: 5, exit: 6, moves: [(move: "jab", clip: "jab")]),"#));
    let mut input = in_move(0.0, JAB, 1, 0.0);
    player.evaluate(&input, source.as_slice(), 0);
    let transitions = player.stats().transitions;
    input.ticks = 1.0;
    input.locomotion.grounded = false;
    player.evaluate(&input, source.as_slice(), 0);
    assert_eq!(player.state(), Name::new("air"));
    assert_eq!(player.stats().transitions, transitions, "it doesn't show");
}

fn layered() -> String {
    document(
        r#"
        masks: [(name: "torso", bones: [(bone: Spine, weight: 1.0)]), (name: "arm", bones: [(bone: RightUpperArm, weight: 1.0)])],
        locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "walk", looping: true))]),
        combat: (decay: 0, exit: 0, moves: [(move: "wave", clip: "wave", mask: "arm")]),
        additive: [(name: "breathing", clip: "breath", mask: "torso", weight: 0.5)],
        reactions: [(name: "flinch", clip: "flinch", mask: "torso", weight: 1.0, fade_out: 10, restart: 4)],
        "#,
    )
}

#[test]
fn an_additive_layer_adds_its_difference_where_its_mask_says() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&layered());
    player.evaluate(&idle(30.0), source.as_slice(), 0);
    // The spine takes half of the clip's turn at 30 of 60 ticks: half of 0.1 rad about Y.
    let spine = turn_of(&player, SPINE);
    assert!(spine.abs_diff_eq(Quat::from_rotation_y(0.05), 2e-3), "{spine}");
    // The hips are walking, and take none of it.
    let mut expected = fixture.pose().rest().to_vec();
    overlay(fixture.clip("walk"), 0.0, &mut expected);
    assert_eq!(player.pose().locals()[usize::from(HIPS)], expected[usize::from(HIPS)]);
}

#[test]
fn a_masked_move_plays_over_locomotion_on_its_joints_only() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&layered().replace(r#"mask: "arm""#, r#"mask: "torso""#));
    player.evaluate(&in_move(30.0, WAVE, 1, 30.0), source.as_slice(), 0);
    // The wave turns the hand, which is under the spine, so it's in the mask.
    let hand = turn_of(&player, HAND);
    assert!(hand.abs_diff_eq(Quat::from_rotation_z(0.5), 2e-3), "{hand}");
    // The hips go on walking below it, which a whole-body move wouldn't sample.
    let mut expected = fixture.pose().rest().to_vec();
    overlay(fixture.clip("walk"), 0.0, &mut expected);
    assert_eq!(player.pose().locals()[usize::from(HIPS)], expected[usize::from(HIPS)]);
}

fn flinch(ticks: f64, count: u32) -> GraphInput {
    GraphInput {
        ticks,
        reaction: ReactionInput { name: Name::new("flinch"), count },
        ..GraphInput::default()
    }
}

/// A reaction starts from nothing, so it needs no transition, plays on the clock and fades to nothing.
#[test]
fn a_reaction_starts_from_nothing_and_fades_to_nothing() {
    let fixture = fixture();
    let source = fixture.source();
    let text = layered()
        .replace(r#"additive: [(name: "breathing", clip: "breath", mask: "torso", weight: 0.5)],"#, "");
    let mut player = fixture.player(&text);
    player.evaluate(&flinch(0.0, 0), source.as_slice(), 0);
    let rest = turn_of(&player, SPINE);
    player.evaluate(&flinch(1.0, 1), source.as_slice(), 0);
    assert_eq!(player.stats().transitions, 0, "nothing to decay");
    // A tick into the 20-tick clip's turn of 0.5 rad about Z.
    let turn = turn_of(&player, SPINE) * rest.inverse();
    assert!(turn.abs_diff_eq(Quat::from_rotation_z(0.025), 2e-3), "{turn}");
    // Fading out, over the last 10 ticks: halfway through them it's half of what it was.
    player.evaluate(&flinch(15.0, 1), source.as_slice(), 0);
    let (_, angle) = (turn_of(&player, SPINE) * rest.inverse()).to_axis_angle();
    assert!((angle - 0.375 * 0.5).abs() < 2e-3, "{angle}");
    // And it's gone at the end, bit for bit.
    player.evaluate(&flinch(21.0, 1), source.as_slice(), 0);
    let rest_pose = fixture.pose().rest().to_vec();
    assert_eq!(player.pose().locals()[usize::from(SPINE)], rest_pose[usize::from(SPINE)]);
    assert_eq!(player.layers.reaction, None);
}

#[test]
fn a_reaction_starting_over_itself_decays_instead_of_jumping() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&layered());
    player.evaluate(&flinch(0.0, 0), source.as_slice(), 0);
    player.evaluate(&flinch(1.0, 1), source.as_slice(), 0);
    player.evaluate(&flinch(8.0, 1), source.as_slice(), 0);
    let before = turn_of(&player, SPINE);
    player.evaluate(&flinch(8.25, 2), source.as_slice(), 0);
    assert_eq!(player.stats().transitions, 1);
    assert!(turn_of(&player, SPINE).angle_between(before) < 0.05, "it doesn't jump");
}

/// Asking for a reaction the document doesn't know does nothing.
#[test]
fn an_unknown_reaction_does_nothing() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&layered());
    player.evaluate(&flinch(0.0, 0), source.as_slice(), 0);
    let input = GraphInput {
        ticks: 1.0,
        reaction: ReactionInput { name: Name::new("sneeze"), count: 1 },
        ..GraphInput::default()
    };
    player.evaluate(&input, source.as_slice(), 0);
    assert_eq!(player.layers.reaction, None);
}

fn blinking() -> String {
    document(
        r#"
        locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "idle", looping: true))]),
        blink: Some((expression: "blink", every: (150, 360), close: 3, hold: 1, open: 7)),
        "#,
    )
}

/// The blink comes and goes on a schedule the document holds, on its own morph target only.
#[test]
fn a_blink_comes_on_its_schedule_on_its_own_target() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&blinking());
    let mut starts = Vec::new();
    let mut peak: f32 = 0.0;
    let mut last = 0.0;
    for tick in 0..6000 {
        player.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
        let weights = player.pose().weights();
        assert_eq!(weights[0], 0.0, "another target");
        assert_eq!(weights[2], 0.25, "another target, at its rest");
        let shut = weights[BLINK_TARGET];
        peak = peak.max(shut);
        if last == 0.0 && shut > 0.0 {
            starts.push(tick);
        }
        last = shut;
    }
    assert_eq!(peak, 1.0, "the eyes shut all the way");
    assert!(starts.len() >= 16, "{starts:?}");
    for pair in starts.windows(2) {
        // Within a tick either way of what the document says, for where a start falls among ticks.
        let gap = pair[1] - pair[0];
        assert!((149..=361).contains(&gap), "{gap} in {starts:?}");
    }
    // Not metronomic.
    let gaps: Vec<i32> = starts.windows(2).map(|pair| pair[1] - pair[0]).collect();
    assert!(gaps.iter().any(|gap| *gap != gaps[0]));
}

/// It's a pure function of the clock: frozen it stays, and two characters blink apart.
#[test]
fn a_blink_is_the_owners_clocks_and_each_characters_own() {
    let fixture = fixture();
    let source = fixture.source();
    let graph = fixture.bind(&fixture.graph(&blinking()));
    let shut_at = |seed: u32, tick: u32| {
        let mut player = GraphPlayer::new(graph.clone(), fixture.pose(), seed);
        player.evaluate(&idle(f64::from(tick)), source.as_slice(), 0);
        player.pose().weights()[BLINK_TARGET]
    };
    // Mid-blink for seed 1, found by looking.
    let tick = (0..1000).find(|&tick| shut_at(1, tick) == 1.0).expect("it blinks");
    assert_eq!(shut_at(1, tick), 1.0, "the same answer however it got here");
    assert!((0..1000).any(|tick| shut_at(1, tick) != shut_at(2, tick)), "two seeds are two schedules");
}

/// `replace` waits for a state to start: until then the old graph plays, then the new one, by name.
#[test]
fn a_replaced_graph_takes_effect_at_the_next_states_start() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&jumping());
    let old = player.graph().clone();
    let tuned =
        jumping().replace(r#"Clip(clip: "air", looping: true)"#, r#"Clip(clip: "run", looping: true)"#);
    let new = fixture.bind(&fixture.graph(&tuned));
    let input = |ticks: f64, grounded: bool| GraphInput {
        ticks,
        locomotion: LocomotionInput { grounded, ..LocomotionInput::default() },
        ..GraphInput::default()
    };
    player.evaluate(&input(0.0, true), source.as_slice(), 0);
    player.replace(new.clone());
    assert!(player.has_replacement());
    // Nothing starts: the old graph plays on, however many frames go by.
    for tick in 1..10 {
        player.evaluate(&input(f64::from(tick), true), source.as_slice(), 0);
        assert!(Arc::ptr_eq(player.graph(), &old), "tick {tick}");
    }
    // A state starts: the new graph, with the state it names as `air`.
    player.evaluate(&input(10.0, false), source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    assert!(!player.has_replacement());
    assert_eq!(player.state(), Name::new("air"));
    // The new `air` plays what the tuning says, after a decay of the old pose.
    for tick in 11..30 {
        player.evaluate(&input(f64::from(tick), false), source.as_slice(), 0);
    }
    let rest = fixture.pose().rest().to_vec();
    let mut expected = rest.clone();
    // It started at the last frame before it, tick 9.
    sample(fixture.clip("run"), (29 - 9) as f32, &rest, &mut expected);
    assert_eq!(bits(player.pose().locals()), bits(&expected));
}

/// A move's start takes a replaced graph. A running move keeps the clip it started with, as a running
/// move keeps its move set's revision, and a state's start under it doesn't take it either.
#[test]
fn a_running_move_keeps_its_clip_until_it_ends() {
    let fixture = fixture();
    let source = fixture.source();
    let text = jumping_with(r#"combat: (decay: 0, exit: 0, moves: [(move: "jab", clip: "jab")]),"#);
    let mut player = fixture.player(&text);
    let tuned = text.replace(r#"(move: "jab", clip: "jab")"#, r#"(move: "jab", clip: "hook")"#);
    let new = fixture.bind(&fixture.graph(&tuned));
    let rest = fixture.pose().rest().to_vec();
    let mut expected = rest.clone();
    player.evaluate(&in_move(0.0, JAB, 1, 0.0), source.as_slice(), 0);
    player.replace(new.clone());
    // Under the jab, a locomotion state starts.
    let mut input = in_move(5.0, JAB, 1, 5.0);
    input.locomotion.grounded = false;
    player.evaluate(&input, source.as_slice(), 0);
    assert_eq!(player.state(), Name::new("air"));
    assert!(player.has_replacement(), "the move hasn't ended");
    sample(fixture.clip("jab"), 5.0, &rest, &mut expected);
    assert_eq!(bits(player.target()), bits(&expected), "still the old table's clip");
    // A new playing of it reads the new table.
    let mut input = in_move(6.0, JAB, 2, 0.0);
    input.locomotion.grounded = false;
    player.evaluate(&input, source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    sample(fixture.clip("hook"), 0.0, &rest, &mut expected);
    assert_eq!(bits(player.target()), bits(&expected));
}

/// A state gone from the new graph falls back on its start. Nothing panics on what it no longer names.
#[test]
fn a_replacement_without_the_playing_state_starts_over_in_its_own() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&jumping());
    let new = fixture.bind(&fixture.graph(&plain()));
    player.evaluate(&idle(0.0), source.as_slice(), 0);
    player.replace(new);
    let input = GraphInput {
        ticks: 1.0,
        locomotion: LocomotionInput { grounded: false, ..LocomotionInput::default() },
        ..GraphInput::default()
    };
    player.evaluate(&input, source.as_slice(), 0);
    assert_eq!(player.state(), Name::new("ground"));
}

/// Standing still with a row for one move, and `plays` for what it stands in: what a replacement adds to.
fn with_one_row(plays: &str) -> String {
    document(&format!(
        r#"
        locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "{plays}", looping: true))]),
        combat: (decay: 8, exit: 12, moves: [(move: "jab", clip: "jab")]),
        "#
    ))
}

/// The pose of `name`'s clip at `position`, over the rest pose.
fn clip_pose(fixture: &Fixture, name: &str, position: f32) -> Vec<Transform> {
    let rest = fixture.pose().rest().to_vec();
    let mut pose = rest.clone();
    sample(fixture.clip(name), position, &rest, &mut pose);
    pose
}

/// A graph replaced to add a row is waiting for that very move: its start is the start that takes it, though
/// the graph playing has never heard of the move. It plays from its first frame, and nothing's unmapped.
#[test]
fn a_replacement_that_adds_a_move_comes_in_at_that_moves_start() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&with_one_row("idle"));
    let new = fixture.bind(&fixture.graph(&plain()));
    player.evaluate(&idle(0.0), source.as_slice(), 0);
    player.replace(new.clone());
    player.evaluate(&idle(1.0), source.as_slice(), 0);
    assert!(player.has_replacement(), "nothing's started");

    player.evaluate(&in_move(2.0, HOOK, 1, 0.0), source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    assert_eq!(bits(player.target()), bits(&clip_pose(&fixture, "hook", 0.0)));
    assert_eq!(player.stats().unmapped_moves, 0);
    assert_eq!(player.stats().transitions, 1, "and it decays from what was drawn");
    player.evaluate(&in_move(3.0, HOOK, 1, 1.0), source.as_slice(), 0);
    assert_eq!(bits(player.target()), bits(&clip_pose(&fixture, "hook", 1.0)));
    assert_eq!(player.stats().transitions, 1);
}

/// Out of a move, into one only the new graph has: one transition, straight to it, with no frame of
/// locomotion between that the new move's first frame would then have to decay from.
#[test]
fn a_cancel_into_a_move_the_replacement_adds_goes_straight_to_it() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&with_one_row("idle"));
    let new = fixture.bind(&fixture.graph(&plain()));
    player.evaluate(&in_move(0.0, JAB, 1, 0.0), source.as_slice(), 0);
    player.replace(new.clone());
    player.evaluate(&in_move(1.0, JAB, 1, 1.0), source.as_slice(), 0);
    assert_eq!(
        bits(player.target()),
        bits(&clip_pose(&fixture, "jab", 1.0)),
        "the running move keeps its clip"
    );
    assert!(player.has_replacement());
    let transitions = player.stats().transitions;

    player.evaluate(&in_move(2.0, HOOK, 2, 0.0), source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    assert_eq!(bits(player.target()), bits(&clip_pose(&fixture, "hook", 0.0)));
    assert_eq!(player.stats().unmapped_moves, 0);
    player.evaluate(&in_move(3.0, HOOK, 2, 1.0), source.as_slice(), 0);
    assert_eq!(player.stats().transitions, transitions + 1, "one, into the hook");
}

/// A move's start takes a replacement that has no row for that move either. It still plays nothing, it's
/// counted, and what the new graph has locomotion playing decays in rather than jumping.
#[test]
fn a_replacement_taken_at_a_move_it_has_no_row_for_decays_in() {
    let fixture = fixture();
    let source = fixture.source();
    let old = with_one_row("idle");
    let no_rows = old.replace(r#"(move: "jab", clip: "jab")"#, "");
    let new = fixture.bind(&fixture.graph(&no_rows.replace(r#"clip: "idle""#, r#"clip: "run""#)));
    // No graph has a row for the hook, and the old graph's row for the jab is gone.
    for name in [HOOK, JAB] {
        let mut player = fixture.player(&old);
        player.evaluate(&idle(0.0), source.as_slice(), 0);
        player.replace(new.clone());
        player.evaluate(&in_move(1.0, name, 1, 0.0), source.as_slice(), 0);
        assert!(Arc::ptr_eq(player.graph(), &new), "{name:?}");
        assert_eq!(player.stats().unmapped_moves, 1, "{name:?}");
        assert_eq!(player.stats().transitions, 1, "{name:?}");
        let hips = fixture.pose().rest()[usize::from(HIPS)].rotation;
        let moved = turn_of(&player, HIPS).angle_between(hips);
        assert!(moved < 0.1, "{name:?}: the hips went {moved} rad toward the run in a frame");
        for tick in 2..40 {
            player.evaluate(&in_move(f64::from(tick), name, 1, f64::from(tick)), source.as_slice(), 0);
        }
        assert_eq!(bits(player.pose().locals()), bits(&clip_pose(&fixture, "run", 9.0)), "{name:?}");
    }
}

/// A move ends in a new one with no row, and the new graph has none for the one that was playing either: the
/// old graph's arm still comes down by a decay.
#[test]
fn a_replacement_without_the_running_move_lets_it_go_by_a_decay() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    let no_rows = with_one_row("idle").replace(r#"(move: "jab", clip: "jab")"#, "");
    let new = fixture.bind(&fixture.graph(&no_rows));
    player.evaluate(&in_move(0.0, JAB, 1, 0.0), source.as_slice(), 0);
    player.evaluate(&in_move(30.0, JAB, 1, 30.0), source.as_slice(), 0);
    player.replace(new.clone());
    let transitions = player.stats().transitions;
    let arm = turn_of(&player, ARM);
    player.evaluate(&in_move(31.0, HOOK, 2, 0.0), source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    assert_eq!(player.stats().transitions, transitions + 1);
    let moved = turn_of(&player, ARM).angle_between(arm);
    assert!(moved < 0.1, "the arm went {moved} rad in a frame");
}

/// A move with no row isn't playing, so nothing keeps a replacement waiting behind it: one that arrives while
/// it runs brings its clip in at the move's own position, which is where its tables are.
#[test]
fn a_replacement_arriving_under_a_move_with_no_row_plays_it_from_where_it_is() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&with_one_row("idle"));
    let new = fixture.bind(&fixture.graph(&plain()));
    player.evaluate(&idle(0.0), source.as_slice(), 0);
    player.evaluate(&in_move(1.0, HOOK, 1, 0.0), source.as_slice(), 0);
    player.evaluate(&in_move(2.0, HOOK, 1, 1.0), source.as_slice(), 0);
    assert_eq!(player.stats().unmapped_moves, 2);
    assert_eq!(player.stats().transitions, 0, "locomotion's playing on");

    player.replace(new.clone());
    player.evaluate(&in_move(3.0, HOOK, 1, 2.0), source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    assert_eq!(bits(player.target()), bits(&clip_pose(&fixture, "hook", 2.0)));
    assert_eq!(player.stats().unmapped_moves, 2);
    assert_eq!(player.stats().transitions, 1);
}

/// A defeated character's last move goes on when the simulation's over with it, and a replacement comes in
/// there: what it names plays on, by a decay.
#[test]
fn a_replacement_taken_by_a_falling_character_decays_in() {
    let fixture = fixture();
    let source = fixture.source();
    let old = a_body_that_falls();
    let tuned = old.replace(r#"(move: "jab", clip: "jab")"#, r#"(move: "jab", clip: "hook")"#);
    let mut player = fixture.player(&old);
    let new = fixture.bind(&fixture.graph(&tuned));
    let combat = Some(CombatInput { name: JAB, serial: 1, position: 10.0 });
    player.evaluate(&defeated(10.0, combat), source.as_slice(), 0);
    player.replace(new.clone());
    let arm = turn_of(&player, ARM);
    player.evaluate(&defeated(11.0, None), source.as_slice(), 0);
    assert!(Arc::ptr_eq(player.graph(), &new));
    assert!(player.is_decaying());
    assert!(turn_of(&player, ARM).angle_between(arm) < 0.1, "not all at once");
}

/// Which playing a move is goes by its name as well as its serial: a party's next member counts its own, so
/// who comes in can start a move under the number the one who left was on.
#[test]
fn another_move_under_the_same_serial_is_a_new_move() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    player.evaluate(&idle(0.0), source.as_slice(), 0);
    player.evaluate(&in_move(1.0, JAB, 3, 0.0), source.as_slice(), 0);
    let transitions = player.stats().transitions;
    player.evaluate(&in_move(2.0, HOOK, 3, 0.0), source.as_slice(), 0);
    assert_eq!(player.stats().transitions, transitions + 1);
    assert_eq!(bits(player.target()), bits(&clip_pose(&fixture, "hook", 0.0)));
}

/// A cut has no decay, and counts: what N5 resets on.
#[test]
fn a_cut_stands_alone_and_says_so() {
    let fixture = fixture();
    let source = fixture.source();
    let rest = fixture.pose().rest().to_vec();
    let mut player = fixture.player(&plain());
    assert_eq!(player.teleports(), 0);
    player.evaluate(&in_move(5.0, JAB, 1, 5.0), source.as_slice(), 0);
    assert_eq!(player.teleports(), 0, "a first frame is no jump");
    let mut input = in_move(6.0, HOOK, 2, 1.0);
    input.cut = true;
    assert!(player.evaluate(&input, source.as_slice(), 0));
    assert_eq!(player.teleports(), 1);
    let mut expected = rest.clone();
    sample(fixture.clip("hook"), 1.0, &rest, &mut expected);
    assert_eq!(bits(player.pose().locals()), bits(&expected), "no decay");
    assert!(!player.is_decaying());
    // The frame after has no velocity to carry either.
    player.evaluate(&in_move(7.0, HOOK, 2, 2.0), source.as_slice(), 0);
    sample(fixture.clip("hook"), 2.0, &rest, &mut expected);
    assert_eq!(bits(player.pose().locals()), bits(&expected));
}

/// A clock that runs back is another clock: the same as a cut.
#[test]
fn a_clock_running_backwards_is_a_cut() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    player.evaluate(&idle(50.0), source.as_slice(), 0);
    player.evaluate(&idle(10.0), source.as_slice(), 0);
    assert_eq!(player.teleports(), 1);
}

#[test]
fn nonsense_input_poisons_nothing() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&blended());
    player.evaluate(&moving(0.0, 1.0), source.as_slice(), 0);
    let mut input = moving(f64::NAN, f32::NAN);
    input.locomotion.direction = f32::INFINITY;
    input.combat = Some(CombatInput { name: JAB, serial: 1, position: f64::NAN });
    player.evaluate(&input, source.as_slice(), 0);
    player.finish();
    assert!(player.pose().model().iter().all(|joint| joint.is_finite()));
    assert!(player.pose().weights().iter().all(|weight| weight.is_finite()));
}

/// A clip that isn't there counts for nothing and is counted: the character keeps going.
#[test]
fn a_clip_gone_missing_is_counted_and_breaks_nothing() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&plain());
    player.evaluate(&in_move(0.0, JAB, 1, 0.0), source.as_slice(), 0);
    let empty: [(AssetId, &ArchivedClip); 0] = [];
    player.evaluate(&in_move(1.0, JAB, 1, 1.0), empty.as_slice(), 1);
    assert_eq!(player.stats().missing_clips, 1);
    player.finish();
    assert!(player.pose().model().iter().all(|joint| joint.is_finite()));
}

/// A new version of the character keeps what's playing: N2's `continue_from`'s job, for graphs.
#[test]
fn continue_from_keeps_what_is_playing() {
    let fixture = fixture();
    let source = fixture.source();
    let mut old = fixture.player(&jumping());
    let input = GraphInput {
        ticks: 3.0,
        locomotion: LocomotionInput { grounded: false, ..LocomotionInput::default() },
        ..GraphInput::default()
    };
    old.evaluate(&idle(0.0), source.as_slice(), 0);
    old.evaluate(&input, source.as_slice(), 0);
    assert_eq!(old.state(), Name::new("air"));
    let mut new = fixture.player(&jumping());
    new.continue_from(&old);
    assert_eq!(new.state(), Name::new("air"));
    assert_eq!(new.teleports(), old.teleports());
    assert!(new.is_decaying());
    let next = GraphInput { ticks: 4.0, ..input };
    assert!(new.evaluate(&next, source.as_slice(), 0));
    old.evaluate(&next, source.as_slice(), 0);
    assert_eq!(bits(new.pose().locals()), bits(old.pose().locals()));
}

#[test]
fn binding_names_what_it_cant_find() {
    let fixture = fixture();
    let rig = CharacterRig::read(&fixture.rig).expect("a valid rig");
    let skeleton = fixture.skeleton();
    let source = fixture.source();
    let good = fixture.graph(&layered());
    assert!(GraphBinding::new(&good, skeleton, Some(rig), 3, source.as_slice()).is_ok());
    // A clip that isn't loaded.
    let few = &source[..1];
    let error = GraphBinding::new(&good, skeleton, Some(rig), 3, few).expect_err("it's missing");
    assert!(matches!(&error, GraphBindError::MissingClip { name, .. } if name == "walk"), "{error}");
    // A clip for another skeleton.
    let other = turning(10, 0, Vec3::X, 0.0, 1.0);
    let other = Clip { joint_count: 2, ..other }.to_cooked().expect("it cooks");
    let mut wrong = source.clone();
    let other_clip = Clip::read(&other).expect("a valid clip");
    wrong.iter_mut().for_each(|(_, clip)| *clip = other_clip);
    let error =
        GraphBinding::new(&good, skeleton, Some(rig), 3, wrong.as_slice()).expect_err("it doesn't fit");
    assert!(matches!(error, GraphBindError::WrongSkeleton { .. }), "{error}");
    // A bone its rig doesn't map, and no rig at all.
    let legs = fixture.graph(&layered().replace("bone: Spine", "bone: Neck"));
    let error =
        GraphBinding::new(&legs, skeleton, Some(rig), 3, source.as_slice()).expect_err("no such bone");
    assert!(matches!(error, GraphBindError::NoSuchBone { bone: HumanBone::Neck, .. }), "{error}");
    let error = GraphBinding::new(&good, skeleton, None, 3, source.as_slice()).expect_err("no rig");
    assert!(matches!(error, GraphBindError::NoSuchBone { .. }), "{error}");
    // A blink's expression it doesn't have.
    let wink = fixture.graph(&blinking().replace(r#"expression: "blink""#, r#"expression: "wink""#));
    let error =
        GraphBinding::new(&wink, skeleton, Some(rig), 3, source.as_slice()).expect_err("no expression");
    assert!(matches!(error, GraphBindError::NoSuchExpression(name) if name == "wink"));
}

/// A jab the character stays down in, with breathing and a blink it would otherwise have.
fn a_body_that_falls() -> String {
    document(
        r#"
        locomotion: (start: "ground", states: [(name: "ground", plays: Clip(clip: "idle", looping: true))]),
        combat: (decay: 0, exit: 12, moves: [(move: "jab", clip: "jab")]),
        additive: [(name: "breathing", clip: "breath", weight: 1.0)],
        blink: Some((expression: "blink", every: (20, 30), close: 3, hold: 1, open: 7)),
        "#,
    )
}

fn defeated(ticks: f64, combat: Option<CombatInput>) -> GraphInput {
    GraphInput { ticks, combat, defeated: true, ..GraphInput::default() }
}

/// A defeated character's move ends in the simulation and its pose doesn't: it plays on to the end of
/// its clip and stays there, with nothing breathing, and from then on it costs what a frozen clock does.
#[test]
fn a_defeated_character_stays_down_when_its_move_ends() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&a_body_that_falls());
    let ended = 20.0;
    for tick in 0..=20 {
        let at = f64::from(tick);
        let combat = Some(CombatInput { name: JAB, serial: 1, position: at });
        assert!(player.evaluate(&defeated(at, combat), source.as_slice(), 0));
        assert_eq!(player.pose().weights()[BLINK_TARGET], 0.0, "no blink at {tick}");
    }
    // The move is over at 20, a third of the way through its clip of 60, and the clip plays on.
    let after = |ticks: f64| defeated(ended + ticks, None);
    assert!(player.evaluate(&after(1.0), source.as_slice(), 0));
    let early = turn_of(&player, ARM);
    for tick in 2..=40 {
        assert!(player.evaluate(&after(f64::from(tick)), source.as_slice(), 0));
        assert_eq!(player.pose().weights()[BLINK_TARGET], 0.0, "no blink at {tick}");
    }
    assert_ne!(turn_of(&player, ARM), early, "the clip goes on");
    // Past the clip's end it's the clip's last keys, under no breathing.
    let mut expected = fixture.pose().rest().to_vec();
    overlay(fixture.clip("jab"), 60.0, &mut expected);
    assert_eq!(bits(player.pose().locals()), bits(&expected));

    // Its clock runs on and nothing's left to do.
    let held = bits(player.pose().locals());
    for tick in 41..400 {
        assert!(!player.evaluate(&after(f64::from(tick)), source.as_slice(), 0), "tick {tick}");
        assert_eq!(bits(player.pose().locals()), held);
    }
    // A new version of a clip still shows, and a cut doesn't get it up.
    assert!(player.evaluate(&after(400.0), source.as_slice(), 1));
    let cut = GraphInput { cut: true, ..after(401.0) };
    assert!(player.evaluate(&cut, source.as_slice(), 1));
    assert_eq!(bits(player.pose().locals()), held);
    assert_eq!(player.stats().unmapped_moves, 0);
}

/// A new round: locomotion takes over as it does after any move.
#[test]
fn a_character_no_longer_defeated_gets_up() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&a_body_that_falls());
    let combat = Some(CombatInput { name: JAB, serial: 1, position: 0.0 });
    assert!(player.evaluate(&defeated(0.0, combat), source.as_slice(), 0));
    assert!(player.evaluate(&defeated(100.0, None), source.as_slice(), 0));
    let down = turn_of(&player, ARM);
    assert!(player.evaluate(&idle(101.0), source.as_slice(), 0));
    assert!(player.is_decaying(), "a decay, not a jump");
    assert!(player.evaluate(&idle(200.0), source.as_slice(), 0));
    assert_ne!(turn_of(&player, ARM), down);
    assert_eq!(turn_of(&player, ARM), fixture.pose().rest()[usize::from(ARM)].rotation);
}

/// Without a row for what felled it there's nothing to stay down in: what was playing doesn't freeze,
/// and the count says what's missing.
#[test]
fn a_defeat_with_no_row_is_counted_and_keeps_no_other_move() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&a_body_that_falls());
    let jab = GraphInput {
        ticks: 0.0,
        combat: Some(CombatInput { name: JAB, serial: 1, position: 0.0 }),
        ..GraphInput::default()
    };
    assert!(player.evaluate(&jab, source.as_slice(), 0));
    let unknown = Some(CombatInput { name: Name::new("fall"), serial: 2, position: 0.0 });
    assert!(player.evaluate(&defeated(1.0, unknown), source.as_slice(), 0));
    assert_eq!(player.stats().unmapped_moves, 1);
    assert!(player.evaluate(&defeated(100.0, None), source.as_slice(), 0));
    assert_eq!(turn_of(&player, ARM), fixture.pose().rest()[usize::from(ARM)].rotation);
}

#[test]
fn a_settled_defeated_rewind_resets_secondary_motion() {
    let fixture = fixture();
    let source = fixture.source();
    let mut player = fixture.player(&a_body_that_falls());
    player.evaluate(
        &defeated(0.0, Some(CombatInput { name: JAB, serial: 1, position: 0.0 })),
        source.as_slice(),
        0,
    );
    player.evaluate(&defeated(100.0, None), source.as_slice(), 0);
    assert!(!player.evaluate(&defeated(101.0, None), source.as_slice(), 0));
    let teleports = player.teleports();
    assert!(player.evaluate(&defeated(50.0, None), source.as_slice(), 0));
    assert_eq!(player.teleports(), teleports + 1);
    assert!(!player.evaluate(&defeated(50.0, None), source.as_slice(), 0));
}

#[test]
fn a_reloaded_joint_identity_does_not_inherit_another_joints_decay() {
    let old_fixture = fixture();
    let mut new_fixture = fixture();
    let mut skeleton = Skeleton::from_cooked(&new_fixture.skeleton).expect("skeleton");
    skeleton.names[usize::from(ARM)] = "different arm".into();
    new_fixture.skeleton = skeleton.to_cooked().expect("cooked");
    let old_source = old_fixture.source();
    let source = new_fixture.source();
    let mut old = old_fixture.player(&plain());
    old.evaluate(&idle(0.0), old_source.as_slice(), 0);
    old.evaluate(&in_move(1.0, JAB, 1, 0.0), old_source.as_slice(), 0);
    assert!(old.is_decaying());
    let mut new = new_fixture.player(&plain());
    new.continue_from(&old);
    assert!(!new.is_decaying());
    assert_eq!(new.state(), old.state());
    assert!(new.evaluate(&in_move(2.0, JAB, 1, 1.0), source.as_slice(), 1));
}

#[test]
fn additive_reference_tracks_the_live_clip_version() {
    let original = fixture();
    let mut replacement = fixture();
    let breath = replacement.clips.iter_mut().find(|(name, _, _)| *name == "breath").expect("breath");
    breath.2 = turning(60, SPINE, Vec3::Y, 0.8, 1.0).to_cooked().expect("cooked");
    let source = original.source();
    let replacement_source = replacement.source();
    let mut player = original.player(&layered());
    player.evaluate(&idle(30.0), source.as_slice(), 0);
    player.evaluate(&idle(30.0), replacement_source.as_slice(), 1);
    // Both clips describe the same 0.2-radian additive motion over their duration, despite
    // different first keys. The reference must change with the live source.
    let turn = turn_of(&player, SPINE);
    assert!(turn.abs_diff_eq(Quat::from_rotation_y(0.05), 2e-3), "{turn}");
}

#[test]
fn references_inactive_during_generation_changes_do_not_reuse_old_data() {
    let original = fixture();
    let mut replacement = fixture();
    let breath = replacement.clips.iter_mut().find(|(name, _, _)| *name == "breath").unwrap();
    breath.2 = turning(60, SPINE, Vec3::Y, 0.8, 1.0).to_cooked().unwrap();
    let original_source = original.source();
    let replacement_source = replacement.source();
    let mut player = original.player(&layered());
    player.evaluate(&idle(27.0), original_source.as_slice(), 0);
    // A version is an identity, not an ordering. Observe two changes while additive layers are
    // inactive, ending at the old numeric value; the inactive reference must still be invalid.
    for (ticks, version) in [(28.0, 1), (29.0, 0)] {
        player.evaluate(
            &GraphInput { ticks, defeated: true, ..GraphInput::default() },
            replacement_source.as_slice(),
            version,
        );
    }
    player.evaluate(&idle(30.0), replacement_source.as_slice(), 0);
    let spine = turn_of(&player, SPINE);
    assert!(spine.abs_diff_eq(Quat::from_rotation_y(0.05), 2e-3), "{spine}");
}

#[test]
fn a_source_revision_keeps_all_u64_bits_on_a_frozen_clock() {
    let original = fixture();
    let mut replacement = fixture();
    let breath = replacement.clips.iter_mut().find(|(name, _, _)| *name == "breath").unwrap();
    breath.2 = turning(60, SPINE, Vec3::Y, 0.8, 1.4).to_cooked().unwrap();
    let original_source = original.source();
    let replacement_source = replacement.source();
    let input = idle(30.0);
    let mut player = original.player(&layered());
    player.evaluate(&input, original_source.as_slice(), 0);
    let revision = 1_u64 << 32;
    assert!(player.evaluate(&input, replacement_source.as_slice(), revision));
    let mut fresh = replacement.player(&layered());
    fresh.evaluate(&input, replacement_source.as_slice(), revision);
    assert_eq!(bits(player.pose().locals()), bits(fresh.pose().locals()));
}
