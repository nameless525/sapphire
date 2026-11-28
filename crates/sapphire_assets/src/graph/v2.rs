//! Graph v2 (M6 A5a, spec §5.13's "Graph v2"): the `.animgraph.ron` document at version 2, a node graph
//! an animator can grow, and its [compiler](compile) to a flat [program] with a static
//! worst-case cost. The cook compiles every graph for the fixed step and writes the program to `.sgraph`
//! (M6 A5c), and `sapphire_body_pose` binds and plays it on either of its maths (M6 A5b). The clips' IDs
//! below are made up.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::AnimGraph",
//!     version: 2,
//!     data: (
//!         // What the graph reads, each bound from a closed list: the controller's locomotion, the
//!         // traversal mode, or a slot the game fills by name.
//!         parameters: [
//!             (name: "velocity_x", from: Locomotion(SmoothedX)),
//!             (name: "velocity_z", from: Locomotion(SmoothedZ)),
//!             (name: "grounded", from: Locomotion(Grounded)),
//!             (name: "mode", from: Traversal(Mode)),
//!             (name: "aim", from: Game("aim")),
//!         ],
//!         // The half-life, in ticks, the smoothed locomotion parameters follow the controller at.
//!         smoothing: 3.0,
//!         clips: [
//!             (name: "idle", source: "11111111-1111-4111-8111-111111111111", clip: "Idle_A"),
//!             (name: "run", source: "11111111-1111-4111-8111-111111111111", clip: "Running_A"),
//!             (name: "fall", source: "11111111-1111-4111-8111-111111111111", clip: "Jump_Idle"),
//!             (name: "jab", source: "22222222-2222-4222-8222-222222222222"),
//!             (name: "jab_behind", source: "33333333-3333-4333-8333-333333333333"),
//!         ],
//!         masks: [(name: "upper", bones: [(bone: Spine, weight: 1.0)])],
//!         // Clips in a group share one phase, counted between one foot's landings.
//!         sync: [(name: "feet", foot: Left, contact: Down)],
//!         // State machines. A state plays a clip, a blend space, an entry blend or another machine.
//!         machines: [
//!             (name: "locomotion", start: "ground", states: [
//!                 (name: "ground", plays: Blend2D((x: "velocity_x", y: "velocity_z", sync: Some("feet"),
//!                     samples: [(clip: "idle", at: (0.0, 0.0), sync: false), (clip: "run", at: (0.0, 4.7))]))),
//!                 (name: "air", plays: Machine("air")),
//!             ], transitions: [
//!                 // A closed grammar: parameters, numbers, `!`, `&&`, `||`, comparisons, `finished`
//!                 // and `finished_at(phase)`. `*` is any state of the machine.
//!                 (from: "ground", to: "air", when: "!grounded", decay: 8),
//!                 (from: "*", to: "ground", when: "grounded && mode == None", decay: 6),
//!             ]),
//!             (name: "air", start: "fall", states: [(name: "fall", plays: Clip(clip: "fall", looping: true))]),
//!         ],
//!         // Bottom to top. The first plays a machine over the whole body.
//!         layers: [
//!             (name: "locomotion", plays: Machine((machine: "locomotion"))),
//!             (name: "combat", plays: Combat((decay: 5, exit: 12, moves: [
//!                 (move: "jab", clip: "jab", variants: [(from: Behind, clip: "jab_behind")]),
//!             ]))),
//!             (name: "breathing", plays: Additive((clip: "idle", mask: Some("upper"), weight: 0.5))),
//!             (name: "reactions", plays: Reactions([(name: "flinch", clip: "jab", weight: 0.7, fade_out: 8)])),
//!         ],
//!         // Named events the graph sends to presentation, from foot contacts or a clip's own events.
//!         events: [(name: "footstep", from: Foot((foot: Left, contact: Down)))],
//!         blink: Some((expression: "blink", every: (150, 360), close: 3, hold: 1, open: 7)),
//!         // Saccades between gaze holds through the rig's look-at, a large one sometimes with a blink.
//!         eyes: Some((reach: (12.0, 6.0), hold: (40, 150), speed: 4.0, blink_chance: 0.3)),
//!     ),
//! )
//! ```
//!
//! # Version 1
//!
//! A version 1 document reads as version 2 ([`convert`]): its locomotion layer becomes a machine named
//! `locomotion`, its conditions expressions over the parameters they read, its combat table, additive
//! layers and reactions layers, and what v1's runtime did by name or by asset ID (the heel-and-ball
//! footwork's clips, the takeoff's carry) says so in the document. The shipped graphs were moved to
//! version 2 so (`sapphire_pipeline upgrade-graphs`, M6 A5c); nothing writes version 1.
//!
//! # Where an editor's layout goes
//!
//! Not here. Node positions and comments live in a sidecar beside the document
//! (`<name>.animgraph.layout.ron`, `sapphire_document::graph_layout`) that no cook reads, so a moved box
//! recooks nothing and no editor type enters this one. A node there is named by its path
//! ([`AnimGraph::node_paths`]), the same path a refusal names.

pub mod compile;
pub mod condition;
pub mod convert;
pub mod program;

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::{SerdeName, Versioned};
use sapphire_serialize::DocumentError;
use serde::{Deserialize, Serialize};

use super::{BlendSample, GraphError};
pub use super::{Blink, EntrySample, Eyes, GraphClip, GraphMask, MaskBone};
use crate::AssetId;
pub use compile::{CompileError, GraphIssue};
pub use program::{GraphProgram, Target};
/// The traversal modes a [`TraversalValue::Mode`] parameter compares against: the controller's own (spec
/// §5.22), numbered as it numbers them.
pub use sapphire_moves::TraversalMode;

/// An animation graph document at version 2. See the [module docs](self).
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
#[type_name = "AnimGraph"]
pub struct AnimGraph {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    /// The half-life, in ticks, that the smoothed locomotion parameters follow the controller's
    /// velocity at: a critically damped spring's. 0 follows it exactly.
    #[serde(default = "default_smoothing")]
    pub smoothing: f32,
    pub clips: Vec<GraphClip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub masks: Vec<GraphMask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sync: Vec<SyncGroup>,
    pub machines: Vec<Machine>,
    pub layers: Vec<Layer>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<GraphEvent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blink: Option<Blink>,
    /// Eyes that move on their own (M6 A22); none, and the eyes rest where the clips leave them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub eyes: Option<Eyes>,
}

impl Versioned for AnimGraph {
    /// 1: M2 N3 (`graph::v1`'s document, read alone).
    /// 2: the node graph (M6 A5a). Version 1 reads, converted ([`convert`]). An upgrade can't refuse
    /// what it decodes, so [`AnimGraph::from_document`] reads a version 1 document through version 1's
    /// own check first: one that check refuses never converts. [`AnimGraph::eyes`] (M6 A22) is optional and
    /// reads as none where it's left out, so it came in at version 2.
    const VERSION: u32 = 2;

    fn upgrade<D: sapphire_reflect::Decoder>(
        version: u32,
        data: D,
    ) -> Result<Self, sapphire_reflect::VersionError<D::Error>> {
        sapphire_reflect::decode_versioned::<super::v1::AnimGraph, D>(version, data)
            .map(|old| convert::convert(&old))
    }
}

pub(crate) fn default_smoothing() -> f32 {
    3.0
}

/// A value the graph reads, and where the app fills it from.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    /// How conditions and blend spaces name it: a lower-case letter, then lower-case letters, digits
    /// and `_`.
    pub name: String,
    pub from: ParameterSource,
}

/// The closed list a parameter is bound from.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum ParameterSource {
    /// The character's movement, from its controller (`LocomotionInput`).
    Locomotion(LocomotionValue),
    /// How it's getting about (spec §5.22's traversal modes).
    Traversal(TraversalValue),
    /// A number the game sets by this name, 0 until it does. At most [`MAX_GAME_SLOTS`].
    Game(String),
}

/// What of a character's movement a parameter reads.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum LocomotionValue {
    /// Meters a second over the ground, as the controller says, unsmoothed.
    Speed,
    /// Which way, relative to the facing, in radians about +Y: 0 ahead, a quarter turn toward +X.
    Direction,
    /// [`Self::Speed`] along the facing: speed times the cosine of direction.
    Forward,
    /// The smoothed velocity's length, m/s.
    SmoothedSpeed,
    /// The smoothed velocity across the facing, m/s, +X the character's left.
    SmoothedX,
    /// The smoothed velocity along the facing, m/s, +Z ahead.
    SmoothedZ,
    /// On the ground (a truth: 1 or 0).
    Grounded,
    /// Landed since the last frame (a truth).
    Landed,
    /// Getting ready to jump, still on the ground (a truth).
    PreparingJump,
}

/// What of a character's traversal a parameter reads.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum TraversalValue {
    /// One of [`TraversalMode`], compared by name: `mode == Climbing`.
    Mode,
}

/// What kind of value a parameter holds, which says how a condition may use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParameterKind {
    /// A number, compared with `<`, `<=`, `>` and `>=`.
    Number,
    /// A truth, read alone or compared with `==` and `!=`.
    Truth,
    /// A [`TraversalMode`], compared with `==` and `!=`.
    Mode,
}

impl ParameterSource {
    #[must_use]
    pub fn kind(&self) -> ParameterKind {
        match self {
            Self::Locomotion(
                LocomotionValue::Grounded | LocomotionValue::Landed | LocomotionValue::PreparingJump,
            ) => ParameterKind::Truth,
            Self::Locomotion(_) | Self::Game(_) => ParameterKind::Number,
            Self::Traversal(TraversalValue::Mode) => ParameterKind::Mode,
        }
    }
}

/// Clips that share one phase: the time since one foot last landed (or lifted), as a fraction of the
/// time to the next, from A1's contact markers. A blend's synced clips, or two states' clips across a
/// transition, keep their feet together this way however long each clip is.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncGroup {
    pub name: String,
    pub foot: Foot,
    pub contact: Contact,
}

/// Which foot.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum Foot {
    Left,
    Right,
}

/// Which of a foot's markers.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum Contact {
    /// It lands.
    Down,
    /// It lifts.
    Up,
}

/// A state machine: one state at a time, moved by transitions.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Machine {
    pub name: String,
    /// The state it starts in, and goes back to when what plays it starts again.
    pub start: String,
    pub states: Vec<State>,
    /// The first that holds wins, in this order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
}

/// A state of a [`Machine`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub name: String,
    pub plays: Motion,
}

/// What a state plays.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum Motion {
    /// One clip from the state's start: holding its last pose, or looping.
    Clip {
        clip: String,
        #[serde(default)]
        looping: bool,
        /// A sync group whose phase it plays at.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sync: Option<String>,
    },
    /// Clips placed along one parameter.
    Blend1D(Blend1D),
    /// Clips placed over two parameters, by gradient bands (v1's blend space).
    Blend2D(Blend2D),
    /// A jump's takeoff: finite clips by the speed it left the ground at, held through the ascent.
    Takeoff(Takeoff),
    /// A touchdown: finite clips by the speed it left at and the speed it lands at.
    Landing(Entry),
    /// Another machine, by name: a nested state machine. Each machine plays in one place.
    Machine(String),
}

/// Clips along one parameter, two neighbours weighing in at a time.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blend1D {
    pub parameter: String,
    pub samples: Vec<Sample1D>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<String>,
}

/// One clip of a [`Blend1D`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample1D {
    pub clip: String,
    /// The parameter's value it plays at as authored. Samples go in increasing order.
    pub at: f32,
}

/// Clips over two parameters: v1's blend space, its axes named.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blend2D {
    pub x: String,
    pub y: String,
    pub samples: Vec<BlendSample>,
    /// The least and most the synced clips' playback may scale by to match the feet to the speed.
    #[serde(default = "default_rate")]
    pub rate: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<String>,
    /// Authored starts and stops over the blend.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub footwork: Option<Footwork>,
}

pub(crate) fn default_rate() -> [f32; 2] {
    [0.5, 2.0]
}

/// Heel-and-ball starts and stops over a ground blend: a brake and a launch for each foot that may be
/// down, played over the hips and spine, and the forward gaits they hand to, each with the share of its
/// cycle the ball stays down.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footwork {
    /// Left support, then right.
    pub brake: [String; 2],
    pub launch: [String; 2],
    /// Slowest first.
    pub gaits: Vec<FootworkGait>,
}

/// A gait a [`Footwork`] hands to.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootworkGait {
    pub clip: String,
    /// The share of its cycle a ball is down, 0 to 1.
    pub contact: f32,
}

/// A takeoff: what an [`Entry`] is, and how it holds.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Takeoff {
    pub standing: String,
    pub moving: Vec<EntrySample>,
    /// Takeoff held through the ascent, with the speed it's going at readied over the descent.
    #[serde(default)]
    pub anticipate: bool,
    /// The outgoing stride's chest and leading arm carried into the ascent.
    #[serde(default)]
    pub carry: bool,
}

/// A standing clip and one to three faster one-shot clips, faded in by speed.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub standing: String,
    pub moving: Vec<EntrySample>,
}

/// What moves a machine from one state to another.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// A state of the machine, or `*` for any.
    pub from: String,
    pub to: String,
    /// A [condition].
    pub when: String,
    /// Ticks the old pose's offset and velocity take to die away.
    pub decay: u32,
    /// What the runtime does with this transition beyond testing its condition. None unless it says
    /// so: a condition never brings a rule with it by its shape.
    #[serde(default, skip_serializing_if = "TransitionRules::is_none")]
    pub rules: TransitionRules,
}

/// The runtime's rules for a transition, each said in the document. v1 keyed these on its condition's
/// variant; a converted transition says which it had ([`convert`]), and a hand-written one has those
/// it names. Spec §5.13's "As built (M6 A5a)" lists them, and the runtime keeps them (M6 A5b).
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransitionRules {
    /// Waits, while a landing's catch is under way, until the landing is ready, unless the
    /// accelerating brake applies (v1's `Moving`, `Stopped`, `ForwardBelow`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub waits_for_landing: bool,
    /// Starts a landing, with its ready time, serial and support (v1's `Landed`, `LandedMoving`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub lands: bool,
    /// Out of a touchdown within 6 ticks of it, fires 2 ticks early: the rebound (v1's
    /// `JumpPreparation(true)`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub rebound: bool,
    /// Hands off a takeoff, whatever state it enters (v1's `AirborneMoving`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub hands_off: bool,
    /// Tested at the tick the pose was drawn, not the frame's (v1's `Finished` out of a touchdown, and
    /// `FinishedAtPhase`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub at_drawn: bool,
    /// Holds through the accelerating catch by the catch's own rule (v1's `FinishedAtPhase`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub catch: bool,
}

impl TransitionRules {
    /// Whether it has no rule: a transition that only tests its condition.
    #[must_use]
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// The name a [`Transition`] gives for any state.
pub const ANY_STATE: &str = "*";

/// A layer, over the ones before it.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub name: String,
    pub plays: LayerMotion,
}

/// What a [`Layer`] plays.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum LayerMotion {
    /// A machine, over what's below by a mask, or added to it.
    Machine(MachineLayer),
    /// The simulation's moves, each a clip at the move's own elapsed ticks. At most one.
    Combat(CombatLayer),
    /// A clip always added over what's below, looping on the owner's clock: the difference between it
    /// and its own first tick.
    Additive(AdditiveLayer),
    /// Clips the game asks for by name, each added once, one at a time. At most one.
    Reactions(Vec<Reaction>),
}

/// A [`LayerMotion::Machine`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MachineLayer {
    pub machine: String,
    /// The joints it takes: every joint without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    #[serde(default, skip_serializing_if = "is_override")]
    pub blend: LayerBlend,
    /// 0 to 1.
    #[serde(default = "full", skip_serializing_if = "is_full")]
    pub weight: f32,
}

/// How a machine layer meets what's below.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum LayerBlend {
    /// Its pose replaces what's below, by the mask's weights and the layer's.
    #[default]
    Override,
    /// Each of its clips' difference from its own first tick is added.
    Additive,
}

fn is_override(blend: &LayerBlend) -> bool {
    *blend == LayerBlend::Override
}

fn full() -> f32 {
    1.0
}

fn is_full(weight: &f32) -> bool {
    *weight == 1.0
}

/// The layer the simulation's moves drive: still keyed to a move's `elapsed`, tick for tick, so a pose
/// and a hit volume agree.
#[derive(Reflect, Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CombatLayer {
    /// Ticks a move's start takes to decay the pose before it, cancels included.
    pub decay: u32,
    /// Ticks what's below takes to decay the pose a move ended on.
    pub exit: u32,
    pub moves: Vec<MoveRow>,
}

/// What plays for a move.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoveRow {
    /// The move's name in its move set.
    #[serde(rename = "move")]
    #[reflect(@SerdeName("move"))]
    pub name: String,
    pub clip: String,
    /// The joints it takes, for an upper-body move over locomotion: every joint without one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    /// Its own decay into it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decay: Option<u32>,
    /// Clips for a move started by a hit from a side, chosen by presentation (spec §5.13's hit
    /// direction). As long as the row's clip, since the move's tables are baked from that one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variants: Vec<Variant>,
}

/// A [`MoveRow`]'s clip for a side.
#[derive(Reflect, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub from: Side,
    pub clip: String,
}

/// Where a hit came from, relative to the character's facing.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
pub enum Side {
    Front,
    Behind,
    Left,
    Right,
}

impl Side {
    pub const ALL: [Self; 4] = [Self::Front, Self::Behind, Self::Left, Self::Right];
}

/// A [`LayerMotion::Additive`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdditiveLayer {
    pub clip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    /// 0 to 1.
    pub weight: f32,
}

/// One of a [`LayerMotion::Reactions`] layer's clips: added from the game's asking, fading out over
/// its last ticks.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reaction {
    pub name: String,
    pub clip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<String>,
    /// 0 to 1.
    pub weight: f32,
    /// Ticks at its end over which its weight falls to nothing.
    #[serde(default)]
    pub fade_out: u32,
    /// Ticks the pose takes to decay if it starts again while it's still playing.
    #[serde(default = "default_restart")]
    pub restart: u32,
}

pub(crate) fn default_restart() -> u32 {
    4
}

/// An event the graph sends to presentation (A6 routes them), from whatever clip weighs in at least
/// `min_weight`.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEvent {
    /// Printable ASCII with no spaces, as a clip's events are.
    pub name: String,
    pub from: EventSource,
    #[serde(default = "half", skip_serializing_if = "is_half")]
    pub min_weight: f32,
}

fn half() -> f32 {
    0.5
}

fn is_half(weight: &f32) -> bool {
    *weight == 0.5
}

/// What fires a [`GraphEvent`].
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum EventSource {
    /// A foot's contact marker, in any clip that has it.
    Foot(FootContact),
    /// A clip's own event of this name (A1's `events:`).
    Clip(String),
}

/// A foot and its marker.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FootContact {
    pub foot: Foot,
    pub contact: Contact,
}

/// The most parameters, and so registers of values.
pub const MAX_PARAMETERS: usize = 32;
/// The most parameters bound to game slots.
pub const MAX_GAME_SLOTS: usize = 8;
/// The most machines.
pub const MAX_MACHINES: usize = 8;
/// The deepest one machine may play inside others, counting itself.
pub const MAX_MACHINE_DEPTH: usize = 4;
/// The most states in one machine.
pub const MAX_STATES: usize = 16;
/// The most transitions in one machine.
pub const MAX_TRANSITIONS: usize = 64;
/// The most layers.
pub const MAX_LAYERS: usize = 8;
/// The most samples in one 1D blend.
pub const MAX_BLEND_1D_SAMPLES: usize = 8;
/// The most sync groups.
pub const MAX_SYNC_GROUPS: usize = 4;
/// The most events.
pub const MAX_EVENTS: usize = 16;
/// The most additive layers (machine layers that add included), each with a reference pose of its own.
pub use super::MAX_ADDITIVE_LAYERS;

impl AnimGraph {
    /// Reads a graph document of either version, a version 1 document converted. Says where malformed
    /// RON is; what's wrong with a document that parsed is [`Self::compile`]'s to say.
    pub fn from_document(text: &str) -> Result<Self, GraphError> {
        // Version 1 reads through its own check, so a document it refuses never converts; anything
        // newer is version 2's to read.
        match super::v1::AnimGraph::from_document(text) {
            Ok(old) => return Ok(convert::convert(&old)),
            Err(GraphError::Document(error)) if matches!(*error, DocumentError::TooNew { .. }) => {}
            Err(error) => return Err(error),
        }
        sapphire_serialize::from_ron::<Self>(text).map_err(|error| match error {
            DocumentError::Parse(spanned) => {
                let line = spanned.span.start.line;
                let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
                GraphError::Malformed {
                    line,
                    column: spanned.span.start.col,
                    message: spanned.code.to_string(),
                    text: quoted.to_owned(),
                }
            }
            other => GraphError::Document(Box::new(other)),
        })
    }

    /// The document, as version 2 writes it.
    pub fn to_document(&self) -> Result<String, DocumentError> {
        sapphire_serialize::to_ron(self)
    }

    /// Fills in each clip's asset ID, which the cook derives from what its document says.
    pub fn resolve(&mut self, mut id: impl FnMut(&GraphClip) -> AssetId) {
        for clip in &mut self.clips {
            clip.id = id(clip);
        }
    }

    /// Everything wrong with it that doesn't depend on a character, each by its path in the document:
    /// what its cook refuses, since it compiles for [`Target::COOKED`] as the cook does, so an editor
    /// shows an event or a game slot as an issue rather than as a cook's failure later.
    #[must_use]
    pub fn check(&self) -> Vec<GraphIssue> {
        match compile::compile_for(self, Target::COOKED) {
            Ok(_) => Vec::new(),
            Err(CompileError { issues }) => issues,
        }
    }

    /// Compiles it to a flat program, or names everything wrong by path. Clip IDs come from
    /// [`Self::resolve`].
    pub fn compile(&self) -> Result<GraphProgram, CompileError> {
        compile::compile(self)
    }

    /// Compiles it to run where `target` says: [`Target::Simulation`] for a graph bound to a
    /// `FixedBody` (`Target::Simulation`), refusing by path what only presentation may run.
    pub fn compile_for(&self, target: Target) -> Result<GraphProgram, CompileError> {
        compile::compile_for(self, target)
    }

    /// The path of every node an editor may place: each machine, each state, each layer. A layout
    /// sidecar names its nodes by these.
    pub fn node_paths(&self) -> impl Iterator<Item = String> + '_ {
        let machines = self.machines.iter().flat_map(|machine| {
            let at = compile::named("machines", &machine.name);
            let states: Vec<String> = machine
                .states
                .iter()
                .map(|state| compile::named(&format!("{at}.states"), &state.name))
                .collect();
            std::iter::once(at).chain(states)
        });
        let layers = self.layers.iter().map(|layer| compile::named("layers", &layer.name));
        machines.chain(layers)
    }
}

#[cfg(test)]
mod tests;
