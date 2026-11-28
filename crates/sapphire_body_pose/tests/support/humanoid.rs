//! A humanoid with every bone, synthetic clips for any graph's clip IDs, the graphs frozen at the
//! switch to version 2 as this crate's test data ([`frozen`]), every shipped graph read from the content
//! tree ([`shipped`], never named: a boss's graph is content, gameplay's `bosses_are_content`) and a
//! scripted session, shared by the graph v2 runtime's unit tests and its allocation test. Its includer
//! names the crate `body` (`use sapphire_body_pose as body`, or `use crate as body` inside it).

#![allow(dead_code, reason = "each includer uses its own part")]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sapphire_assets::AssetId;
use sapphire_assets::clip::{
    ArchivedClip, Clip, DEFAULT_RATE, PackedRotation, RotationTrack, WeightRange, WeightTrack,
};
use sapphire_assets::graph::v2::program::GraphProgram;
use sapphire_assets::graph::{clip_id, v2};
use sapphire_assets::rig::{
    ArchivedCharacterRig, CharacterRig, Expression, ExpressionOverride, ExpressionPreset, HumanBone,
    HumanoidJoint, LookAt, LookAtKind, LookAtRange, MorphTargetWeight,
};
use sapphire_assets::skeleton::{ArchivedSkeleton, Joint, JointTransform, Skeleton};
use sapphire_core::Name;
use sapphire_math::Vec3;
use sapphire_math::det::quat_from_axis_angle;
use sapphire_serialize::cooked::AlignedVec;

use super::body::{CharacterPose, CombatInput, GraphBinding, GraphInput, LocomotionInput, ReactionInput};

pub const MORPHS: usize = 3;

/// A humanoid skeleton of every bone, each under the one it hangs from.
pub fn humanoid_joints() -> Vec<Joint> {
    let index = |bone: HumanBone| HumanBone::ALL.iter().position(|&found| found == bone).map(|i| i as u16);
    HumanBone::ALL
        .iter()
        .map(|&bone| {
            let name = bone.name();
            let side = if name.starts_with("left") { 1.0 } else { -1.0 };
            let sided = |left: HumanBone, right: HumanBone| if side > 0.0 { left } else { right };
            use HumanBone as B;
            let (parent, translation) = match bone {
                B::Hips => (None, [0.0, 0.95, 0.0]),
                B::Spine => (Some(B::Hips), [0.0, 0.1, 0.0]),
                B::Chest => (Some(B::Spine), [0.0, 0.12, 0.0]),
                B::UpperChest => (Some(B::Chest), [0.0, 0.12, 0.0]),
                B::Neck => (Some(B::UpperChest), [0.0, 0.12, 0.0]),
                B::Head => (Some(B::Neck), [0.0, 0.1, 0.0]),
                B::LeftEye | B::RightEye => (Some(B::Head), [side * 0.03, 0.06, 0.08]),
                B::Jaw => (Some(B::Head), [0.0, -0.02, 0.06]),
                B::LeftUpperLeg | B::RightUpperLeg => (Some(B::Hips), [side * 0.1, -0.05, 0.0]),
                B::LeftLowerLeg | B::RightLowerLeg => {
                    (Some(sided(B::LeftUpperLeg, B::RightUpperLeg)), [0.0, -0.42, 0.0])
                }
                B::LeftFoot | B::RightFoot => {
                    (Some(sided(B::LeftLowerLeg, B::RightLowerLeg)), [0.0, -0.42, 0.0])
                }
                B::LeftToes | B::RightToes => (Some(sided(B::LeftFoot, B::RightFoot)), [0.0, -0.06, 0.12]),
                B::LeftShoulder | B::RightShoulder => (Some(B::UpperChest), [side * 0.05, 0.1, 0.0]),
                B::LeftUpperArm | B::RightUpperArm => {
                    (Some(sided(B::LeftShoulder, B::RightShoulder)), [side * 0.12, 0.0, 0.0])
                }
                B::LeftLowerArm | B::RightLowerArm => {
                    (Some(sided(B::LeftUpperArm, B::RightUpperArm)), [side * 0.27, 0.0, 0.0])
                }
                B::LeftHand | B::RightHand => {
                    (Some(sided(B::LeftLowerArm, B::RightLowerArm)), [side * 0.25, 0.0, 0.0])
                }
                // A finger hangs from its hand.
                _ => (Some(sided(B::LeftHand, B::RightHand)), [side * 0.05, 0.0, 0.02]),
            };
            Joint {
                name: name.to_owned(),
                parent: parent.and_then(index),
                rest: JointTransform { translation, ..JointTransform::IDENTITY },
            }
        })
        .collect()
}

/// The humanoid's rig: every bone, a blink, and a look-at that turns its eye bones as the heroine's does,
/// 10° of eye at 90° of gaze, so a shipped graph's eyes bind.
pub fn humanoid_rig() -> CharacterRig {
    let mut humanoid: Vec<HumanoidJoint> = HumanBone::ALL
        .iter()
        .enumerate()
        .map(|(joint, &bone)| HumanoidJoint { bone, joint: joint as u16 })
        .collect();
    humanoid.sort_by_key(|entry| entry.bone);
    CharacterRig {
        joint_count: HumanBone::ALL.len() as u32,
        morph_target_count: MORPHS as u32,
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
        look_at: Some(LookAt {
            offset_from_head: [0.0, 0.06, 0.08],
            kind: LookAtKind::Bone,
            horizontal_inner: HEROINE_RANGE,
            horizontal_outer: HEROINE_RANGE,
            vertical_down: HEROINE_RANGE,
            vertical_up: HEROINE_RANGE,
        }),
        ..CharacterRig::default()
    }
}

/// The heroine's range maps, each way.
pub const HEROINE_RANGE: LookAtRange = LookAtRange { input_max_degrees: 90.0, output_scale: 10.0 };

/// A clip of its own for `id`: three joints turning on keys its bytes choose, and on some a morph.
pub fn synthetic(id: AssetId) -> Clip {
    let bytes = id.to_bytes();
    let joints = HumanBone::ALL.len() as u16;
    let ticks = 16 + u16::from(bytes[0] % 48);
    let angle = |byte: u8| (f32::from(byte) / 255.0 - 0.5) * 1.6;
    let axes = [Vec3::X, Vec3::Y, Vec3::Z];
    let mut chosen: Vec<u16> = [0, 1 + u16::from(bytes[1]) % 24, 1 + u16::from(bytes[2]) % 24].to_vec();
    chosen.sort_unstable();
    chosen.dedup();
    let mut rotations = Vec::new();
    let mut rotation_ticks = Vec::new();
    let mut rotation_keys = Vec::new();
    for (index, &joint) in chosen.iter().enumerate() {
        let axis = axes[usize::from(bytes[3 + index]) % 3];
        let key = |byte: u8| PackedRotation::encode(quat_from_axis_angle(axis, angle(byte)).to_array());
        rotations.push(RotationTrack { joint, count: 3, first: rotation_keys.len() as u32 });
        rotation_ticks.extend([0, ticks / 2, ticks]);
        rotation_keys.extend([key(bytes[6 + index]), key(bytes[9 + index]), key(bytes[12 + index])]);
    }
    let mut clip = Clip {
        name: format!("{id}"),
        skeleton: Some(AssetId::from_bytes([9; 16])),
        joint_count: u32::from(joints),
        morph_target_count: MORPHS as u32,
        rate: DEFAULT_RATE,
        ticks,
        rotations,
        rotation_ticks,
        rotation_keys,
        ..Clip::default()
    };
    if bytes[15].is_multiple_of(2) {
        let range = WeightRange::enclosing([0.0, 1.0]);
        clip.weights = vec![WeightTrack { target: 0, count: 2, first: 0, range }];
        clip.weight_ticks = vec![0, ticks];
        clip.weight_keys = vec![range.encode(0.0), range.encode(1.0)];
    }
    clip
}

/// A humanoid's cooked skeleton and rig, and a clip for each ID asked for.
pub struct Humanoid {
    pub skeleton: AlignedVec,
    pub rig: AlignedVec,
    pub clips: Vec<(AssetId, AlignedVec)>,
}

impl Humanoid {
    pub fn new(ids: impl IntoIterator<Item = AssetId>) -> Self {
        let mut clips: Vec<(AssetId, AlignedVec)> = Vec::new();
        for id in ids {
            if clips.iter().all(|(found, _)| *found != id) {
                clips.push((id, synthetic(id).to_cooked().expect("it cooks")));
            }
        }
        Self {
            skeleton: Skeleton::new(&humanoid_joints()).to_cooked().expect("it cooks"),
            rig: humanoid_rig().to_cooked().expect("it cooks"),
            clips,
        }
    }

    pub fn skeleton(&self) -> &ArchivedSkeleton {
        Skeleton::read(&self.skeleton).expect("a valid skeleton")
    }

    pub fn rig(&self) -> &ArchivedCharacterRig {
        CharacterRig::read(&self.rig).expect("a valid rig")
    }

    pub fn source(&self) -> Vec<(AssetId, &ArchivedClip)> {
        self.clips.iter().map(|(id, bytes)| (*id, Clip::read(bytes).expect("a valid clip"))).collect()
    }

    pub fn pose(&self) -> CharacterPose {
        CharacterPose::new(self.skeleton(), &[], &[], vec![0.0, 0.0, 0.25]).with_morph_names(vec![
            "smile".into(),
            "blink".into(),
            "rest".into(),
        ])
    }

    pub fn bind(&self, program: &GraphProgram) -> Arc<GraphBinding> {
        let source = self.source();
        Arc::new(
            GraphBinding::from_program(program, self.skeleton(), Some(self.rig()), MORPHS, source.as_slice())
                .unwrap_or_else(|error| panic!("{error}")),
        )
    }
}

/// The graphs frozen as `tests/fixtures/graphs/<name>.animgraph.ron`: the seven shipped at the switch to
/// version 2 (M6 A5c), as version 2 documents, copied once and never edited with the content. The runtime's
/// pins are held on these, so a content edit never moves one.
pub const FROZEN: [&str; 7] = ["crowd", "warrior", "hero", "boss", "lancer", "wren", "heroine"];

/// Each [`FROZEN`] graph's document, by name, in that order.
pub fn frozen() -> Vec<(String, String)> {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graphs");
    FROZEN
        .iter()
        .map(|name| {
            let path = folder.join(format!("{name}.animgraph.ron"));
            let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{name}: {error}"));
            ((*name).to_owned(), text)
        })
        .collect()
}

/// Every shipped graph's document, by its path in the content folder, written with `/`.
pub fn shipped() -> Vec<(String, String)> {
    let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content");
    let mut found = Vec::new();
    let mut folders: Vec<PathBuf> = vec![content.clone()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).expect("a content folder") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                folders.push(path);
            } else if path.to_string_lossy().ends_with(".animgraph.ron") {
                let text = std::fs::read_to_string(&path).expect("a graph");
                // Written with `/` on every OS, as a test names them.
                let name = path
                    .strip_prefix(&content)
                    .expect("in content")
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                found.push((name, text));
            }
        }
    }
    found.sort();
    assert!(found.len() >= 7, "every shipped graph: {}", found.len());
    found
}

/// A shipped graph as the cook reads it, at version 2, its clips resolved as the cook resolves them.
pub fn read(text: &str) -> v2::AnimGraph {
    let mut graph = v2::AnimGraph::from_document(text).expect("a shipped graph reads");
    graph.resolve(clip_id);
    graph
}

/// A frame of a script: what the controller says, and what the stage passes beside it.
#[derive(Clone, Copy)]
pub struct Step {
    pub input: GraphInput,
    pub turn: Option<usize>,
    pub ground: Option<usize>,
    pub gather: Option<bool>,
    pub replace: bool,
}

/// A scripted session, built a frame at a time.
pub struct Script {
    pub steps: Vec<Step>,
    pub ticks: f64,
    pub serial: u32,
    pub reactions: u32,
}

pub const STILL: LocomotionInput = LocomotionInput {
    speed: 0.0,
    direction: 0.0,
    grounded: true,
    landed: false,
    preparing_jump: false,
    moving_jump_preparation: None,
    departure_speed: None,
    touchdown_forward_speed: None,
};

pub fn moving_at(speed: f32, direction: f32) -> LocomotionInput {
    LocomotionInput { speed, direction, ..STILL }
}

impl Script {
    pub fn new() -> Self {
        Self { steps: Vec::new(), ticks: 0.0, serial: 0, reactions: 0 }
    }

    pub fn frame(&mut self, dt: f64, locomotion: LocomotionInput) -> &mut Step {
        self.ticks += dt;
        let ground = locomotion.grounded.then_some((self.steps.len() / 7) % 2);
        self.steps.push(Step {
            input: GraphInput { ticks: self.ticks, locomotion, ..GraphInput::default() },
            turn: None,
            ground,
            gather: None,
            replace: false,
        });
        self.steps.last_mut().expect("just pushed")
    }

    pub fn hold(&mut self, frames: usize, locomotion: LocomotionInput) {
        for _ in 0..frames {
            self.frame(1.0, locomotion);
        }
    }

    /// From `from` to `to` m/s over `frames`, straight ahead.
    pub fn ramp(&mut self, frames: usize, from: f32, to: f32, direction: f32) {
        for frame in 1..=frames {
            let t = frame as f32 / frames as f32;
            self.frame(1.0, moving_at(from + (to - from) * t, direction));
        }
    }

    /// A jump from `speed`: preparing, airborne for `air` frames, then landing at `land` m/s.
    pub fn jump(&mut self, speed: f32, departure: f32, air: usize, land: f32) {
        for frame in 0..4 {
            self.frame(
                1.0,
                LocomotionInput {
                    preparing_jump: true,
                    moving_jump_preparation: (speed > 0.0).then_some(frame as f32 / 4.0),
                    ..moving_at(speed, 0.0)
                },
            );
        }
        let flying =
            LocomotionInput { grounded: false, departure_speed: Some(departure), ..moving_at(speed, 0.0) };
        self.hold(air, flying);
        self.frame(
            1.0,
            LocomotionInput {
                landed: true,
                touchdown_forward_speed: Some(land),
                departure_speed: Some(departure),
                ..moving_at(land, 0.0)
            },
        );
    }

    pub fn play(&mut self, name: Name, frames: usize, from: f64) {
        self.serial += 1;
        let serial = self.serial;
        for frame in 0..frames {
            let step = self.frame(1.0, STILL);
            step.input.combat = Some(CombatInput { name, serial, position: from + frame as f64 });
        }
    }

    pub fn react(&mut self, name: Name) {
        self.reactions += 1;
        let count = self.reactions;
        let step = self.frame(1.0, STILL);
        step.input.reaction = ReactionInput { name, count };
    }

    /// Carries the last reaction on frames that follow, as the controller does.
    pub fn finish(mut self) -> Vec<Step> {
        let mut reaction = ReactionInput::default();
        for step in &mut self.steps {
            if step.input.reaction.count == 0 {
                step.input.reaction = reaction;
            } else {
                reaction = step.input.reaction;
            }
        }
        self.steps
    }
}

/// The session every graph plays: locomotion with starts, stops and turns, a frozen and a fractional
/// clock, every kind of jump and landing, a combo of the graph's own moves with hitstop and cancels,
/// reactions, a cut, a defeat and a replacement of the graph under it.
pub fn session(graph: &v2::AnimGraph) -> Vec<Step> {
    let mut script = Script::new();
    script.hold(20, STILL);
    // Away, up to a run, then a forward stop and a start again.
    script.ramp(3, 0.0, 2.0, 0.0);
    script.ramp(30, 2.0, 6.0, 0.0);
    script.hold(20, moving_at(6.0, 0.0));
    // Released, so the forward stop brakes on the heel and ball.
    for frame in 0..4 {
        let step = script.frame(1.0, moving_at(4.5 - 1.5 * frame as f32, 0.0));
        step.gather = Some(true);
    }
    script.hold(30, STILL);
    script.frame(1.0, STILL).gather = Some(false);
    script.ramp(3, 0.0, 3.0, 0.0);
    script.hold(30, moving_at(3.0, 0.0));
    // Turns: to a strafe, with the support foot handed over, then back and ahead again.
    for frame in 1..=10 {
        let step = script.frame(1.0, moving_at(3.0, frame as f32 * 0.157));
        step.turn = (frame % 4 == 0).then_some(frame % 2);
    }
    script.hold(15, moving_at(3.0, 1.57));
    script.hold(15, moving_at(1.2, std::f32::consts::PI));
    script.hold(15, moving_at(3.0, -1.57));
    let step = script.frame(1.0, moving_at(3.0, 0.0));
    step.turn = Some(0);
    script.hold(10, moving_at(3.0, 0.0));
    // A frozen clock, then a fractional one.
    for _ in 0..4 {
        script.frame(0.0, moving_at(3.5, 0.2));
    }
    for frame in 0..12 {
        script.frame(if frame % 2 == 0 { 0.5 } else { 1.7 }, moving_at(3.0, 0.0));
    }
    // Released out of a strafe, too slow for footwork: the grounded gather.
    script.hold(10, moving_at(3.0, 1.57));
    script.frame(1.0, moving_at(0.22, 1.57)).gather = Some(true);
    script.hold(20, moving_at(0.22, 1.57));
    script.frame(1.0, STILL).gather = Some(false);
    script.ramp(4, 3.0, 0.0, 0.0);
    script.hold(25, STILL);
    // Preparing and thinking better of it.
    script.hold(3, LocomotionInput { preparing_jump: true, ..STILL });
    script.hold(10, STILL);
    // A standing jump, landing still and finishing the landing.
    script.jump(0.0, 0.0, 25, 0.0);
    script.hold(70, STILL);
    // A running jump, landing at a run.
    script.hold(20, moving_at(3.0, 0.0));
    script.jump(3.0, 3.0, 20, 3.0);
    script.hold(60, moving_at(3.0, 0.0));
    // A short hop into a fast touchdown: the accelerating catch.
    script.ramp(4, 3.0, 0.0, 0.0);
    script.hold(20, STILL);
    script.jump(0.0, 0.3, 20, 6.0);
    script.hold(60, moving_at(6.0, 0.0));
    script.ramp(5, 6.0, 0.0, 0.0);
    script.hold(20, STILL);
    // Landing, and off again at once.
    script.jump(0.0, 0.0, 15, 0.0);
    script.hold(2, STILL);
    script.hold(12, LocomotionInput { grounded: false, departure_speed: Some(0.0), ..STILL });
    // Down without a landing.
    script.hold(20, STILL);
    // Landing into a rebound.
    script.jump(0.0, 0.0, 15, 0.0);
    script.hold(2, STILL);
    script.hold(3, LocomotionInput { preparing_jump: true, ..STILL });
    script.hold(12, LocomotionInput { grounded: false, departure_speed: Some(0.0), ..STILL });
    script.frame(1.0, LocomotionInput { landed: true, ..STILL });
    script.hold(60, STILL);
    // Off a ledge at a walk.
    script.hold(10, moving_at(2.0, 0.0));
    script.hold(10, LocomotionInput { grounded: false, ..moving_at(2.0, 0.0) });
    script.frame(1.0, LocomotionInput { landed: true, ..moving_at(2.0, 0.0) });
    script.hold(40, moving_at(2.0, 0.0));
    script.ramp(4, 2.0, 0.0, 0.0);
    script.hold(10, STILL);
    // A combo of the graph's own moves, each cancelling the last, with hitstop on the third.
    let moves: Vec<Name> = graph
        .layers
        .iter()
        .filter_map(|layer| match &layer.plays {
            v2::LayerMotion::Combat(combat) => Some(&combat.moves),
            _ => None,
        })
        .flatten()
        .map(|row| Name::intern(&row.name))
        .collect();
    for (index, &name) in moves.iter().enumerate() {
        script.play(name, 9, 0.0);
        if index == 2 {
            let held = script.steps.last().expect("a frame").input;
            for _ in 0..4 {
                let step = script.frame(0.0, STILL);
                step.input.combat = held.combat;
            }
        }
    }
    // The last to its end and past it, then locomotion takes over.
    if let Some(&last) = moves.first() {
        script.play(last, 90, 0.0);
    }
    script.hold(20, STILL);
    // Reactions: one, and another over it.
    let reactions = graph.layers.iter().filter_map(|layer| match &layer.plays {
        v2::LayerMotion::Reactions(rows) => Some(rows),
        _ => None,
    });
    for row in reactions.flatten() {
        script.react(Name::intern(&row.name));
        script.hold(5, STILL);
        script.react(Name::intern(&row.name));
        script.hold(40, moving_at(1.0, 0.0));
    }
    // A replacement under a moving character, and a cut.
    script.hold(5, moving_at(2.0, 0.0));
    script.frame(1.0, moving_at(2.0, 0.0)).replace = true;
    script.hold(5, moving_at(2.0, 0.0));
    script.frame(1.0, moving_at(2.0, 0.0)).input.cut = true;
    script.hold(10, moving_at(2.0, 0.0));
    script.hold(5, STILL);
    // Out of the fight, and back for a new round.
    if let Some(&last) = moves.last() {
        script.play(last, 10, 0.0);
    }
    let ending = Some(CombatInput {
        name: moves.last().copied().unwrap_or(Name::new("")),
        serial: script.serial,
        position: 10.0,
    });
    for _ in 0..30 {
        let step = script.frame(1.0, STILL);
        step.input.defeated = true;
        step.input.combat = ending;
    }
    script.hold(20, STILL);
    script.finish()
}
