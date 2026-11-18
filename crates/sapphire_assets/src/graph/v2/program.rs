//! A graph compiled: flat tables a runtime walks every frame with no names to look up and nothing to
//! allocate, over two fixed register files, one of values and one of poses.
//!
//! # Registers
//!
//! **Values:** one `f32` a parameter, by its index in [`GraphProgram::parameters`], which the app fills
//! each frame: a truth is 1 or 0, a traversal mode its number in [`TraversalMode`].
//! **Poses:** [`GraphCost::registers`] of them, at most [`MAX_REGISTERS`]. Register 0 is the output. An
//! op writes the register it names and may use the ones above it as scratch, as many as its kind needs
//! ([`PoseOp::scratch`]), so the file is a stack whose depth the compiler knows.
//!
//! # A frame
//!
//! The ops of [`GraphProgram::layers`] run in order. A [`PoseOp::Machine`] runs its machine's active
//! state's ops ([`GraphProgram::state_ops`]) into its register, which, for a nested machine, runs that
//! machine's in turn. A machine's transitions ([`GraphProgram::transitions`]) are tried in order from
//! the active state (or any, `from: None`), each by [`GraphProgram::holds`] over the value registers.
//!
//! # Cost
//!
//! [`GraphCost`] is the program's worst case, worked out once at compile time, **per evaluation of the
//! layers**: the most clips one evaluation can sample, registers, ops, condition steps and nesting. No
//! evaluation can exceed it. A frame can evaluate more than once, and v1, which a converted graph must
//! match, does on three kinds of frame:
//!
//! - a frame that starts a decay (a transition, a move's start or end, a reaction's restart, a gather,
//!   a footwork start) evaluates its target three times: now, where the pose was drawn, and a tick on;
//! - a landing's or a turn's hand-off to a blend scores gait phases, each with a whole locomotion
//!   evaluation (23 phases for a landing, up to 11 phases at two evaluations each for a turn);
//! - the first frame of an additive layer, or of each new reaction, samples its clip's first tick for
//!   its reference.
//!
//! The heroine's converted graph is 10 an evaluation; her worst frame by v1's code is a landing's
//! hand-off, which starts a decay: 3 × 10 + 23 × 7 (her 7-clip ground) = 191, and 345 should a turn's
//! hand-off fall on the same frame (11 × 2 × 7 more), plus one a new reference. A5b's runtime decides
//! whether v2 keeps phase matching by whole evaluations; no field counts the multipliers until it does.

use bevy_reflect::TypePath;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use super::condition::{MAX_CONDITION_DEPTH, MAX_CONDITION_OPS, depth};
use super::{
    Contact, Foot, LayerBlend, MAX_ADDITIVE_LAYERS, MAX_BLEND_1D_SAMPLES, MAX_EVENTS, MAX_LAYERS,
    MAX_MACHINE_DEPTH, MAX_MACHINES, MAX_PARAMETERS, MAX_STATES, MAX_SYNC_GROUPS, MAX_TRANSITIONS,
    ParameterSource, Side, TransitionRules, TraversalMode,
};
use crate::AssetId;
use crate::graph::{
    Blink, GraphError, GraphMask, MAX_BLEND_SAMPLES, MAX_GRAPH_CLIPS, MAX_GRAPH_MOVES, MAX_MASK_BONES,
    MAX_MASKS, MAX_REACTIONS,
};

/// The most pose registers a program may use.
pub const MAX_REGISTERS: usize = 8;
/// The most clips one evaluation of one character's layers may sample, worst case: what a graph's cost
/// is capped at. A frame may evaluate more than once (the module's "Cost" says when).
///
/// Spec §5.13's decision says 8. The heroine's shipped graph needs 10 an evaluation converted as it
/// plays today (a 7-clip ground blend with its footwork's phrase, or a 4-clip landing sampled again for
/// its catch, then breathing and a reaction), since v1's 2D blend can weigh every sample in at once; 12
/// holds every shipped graph with room for two more samples, until the maintainer settles it.
pub const MAX_CLIPS_SAMPLED: usize = 12;

/// A graph compiled. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::assets"]
pub struct GraphProgram {
    /// The value registers, by index.
    pub parameters: Vec<ProgramParameter>,
    /// The smoothed locomotion parameters' half-life, in ticks.
    pub smoothing: f32,
    pub clips: Vec<ProgramClip>,
    /// By humanoid bone, as the document has them: a character's joints are its binding's.
    pub masks: Vec<GraphMask>,
    pub sync: Vec<ProgramSync>,
    pub machines: Vec<ProgramMachine>,
    /// Every machine's states, each machine's together.
    pub states: Vec<ProgramState>,
    /// Every machine's transitions, each machine's together, in its document's order.
    pub transitions: Vec<ProgramTransition>,
    /// Every condition's ops, each transition's together.
    pub conditions: Vec<ConditionOp>,
    /// Every state's ops, then the layers'.
    pub ops: Vec<PoseOp>,
    /// The ops a frame runs, bottom layer first.
    pub layers: Span,
    pub blends: Vec<ProgramBlend>,
    /// Every blend's samples, each blend's together.
    pub samples: Vec<ProgramSample>,
    pub entries: Vec<ProgramEntry>,
    pub footwork: Vec<ProgramFootwork>,
    /// The combat layer's, if it has one.
    pub combat: Option<ProgramCombat>,
    /// By name, for a binary search.
    pub moves: Vec<ProgramMove>,
    pub additive: Vec<ProgramAdditive>,
    pub reactions: Vec<ProgramReaction>,
    pub events: Vec<ProgramEvent>,
    pub blink: Option<Blink>,
    pub cost: GraphCost,
}

impl Versioned for GraphProgram {
    /// 1: M6 A5a.
    const VERSION: u32 = 1;
}

/// A run of a table: `start..start + len`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct Span {
    pub start: u16,
    pub len: u16,
}

impl Span {
    #[must_use]
    pub fn range(self) -> std::ops::Range<usize> {
        usize::from(self.start)..usize::from(self.start) + usize::from(self.len)
    }

    fn fits(self, len: usize) -> bool {
        self.range().end <= len
    }
}

/// A value register's name and source.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramParameter {
    pub name: String,
    pub from: ParameterSource,
}

/// A clip, by name for messages and by ID for loading.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramClip {
    pub name: String,
    pub id: AssetId,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramSync {
    pub name: String,
    pub foot: Foot,
    pub contact: Contact,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramMachine {
    pub name: String,
    /// A state in [`GraphProgram::states`].
    pub start: u16,
    pub states: Span,
    pub transitions: Span,
    /// The pose register its states write.
    pub register: u8,
    /// 1 for a layer's machine, one more for each machine it plays inside.
    pub depth: u8,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramState {
    pub name: String,
    pub machine: u16,
    pub ops: Span,
    /// Whether what it plays can end, so `finished` can hold in it.
    pub finishes: bool,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramTransition {
    /// A state in [`GraphProgram::states`], or `None` for any of its machine's.
    pub from: Option<u16>,
    pub to: u16,
    /// Ticks.
    pub decay: u16,
    pub condition: Span,
    /// The value registers its condition reads, a bit each: what a runtime needs to tell a landing's
    /// condition (one reading `landed`) or a speed's from the rest without parsing it again.
    pub reads: u32,
    /// Whether its condition asks whether the state has finished.
    pub finished: bool,
    /// `finished_at`'s phase: where the next state's gait starts.
    pub phase: Option<f32>,
    /// What the runtime does with it beyond its condition, as the document says.
    pub rules: TransitionRules,
}

/// A condition's step, postfix over a stack of truths.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ConditionOp {
    Constant(bool),
    /// A truth parameter.
    Truth(u8),
    /// A number parameter against a number.
    Compare {
        parameter: u8,
        comparison: Comparison,
        value: f32,
    },
    /// Two parameters of a kind.
    Between {
        left: u8,
        comparison: Comparison,
        right: u8,
    },
    /// A traversal mode parameter is this mode.
    Mode {
        parameter: u8,
        mode: u8,
    },
    /// The state's clip has played to its end.
    Finished,
    Not,
    And,
    Or,
}

/// How a [`ConditionOp`] compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum Comparison {
    Less,
    AtMost,
    Greater,
    AtLeast,
    Equal,
    NotEqual,
}

impl Comparison {
    /// The same test with its sides swapped.
    #[must_use]
    pub fn flipped(self) -> Self {
        match self {
            Self::Less => Self::Greater,
            Self::AtMost => Self::AtLeast,
            Self::Greater => Self::Less,
            Self::AtLeast => Self::AtMost,
            other => other,
        }
    }

    #[must_use]
    pub fn holds(self, left: f32, right: f32) -> bool {
        match self {
            Self::Less => left < right,
            Self::AtMost => left <= right,
            Self::Greater => left > right,
            Self::AtLeast => left >= right,
            Self::Equal => left == right,
            Self::NotEqual => left != right,
        }
    }
}

/// A step of a frame's pose work.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PoseOp {
    /// One clip from the state's start, held at its end or looping, at a sync group's phase if it has one.
    Clip { clip: u16, looping: bool, sync: Option<u8>, into: u8 },
    /// A blend space, in [`GraphProgram::blends`].
    Blend { blend: u16, into: u8 },
    /// A takeoff or a landing, in [`GraphProgram::entries`].
    Entry { entry: u16, into: u8 },
    /// A machine's active state's ops, into the machine's register.
    Machine { machine: u16, into: u8 },
    /// `from` over `onto`, by a mask and a weight.
    Layer { from: u8, onto: u8, mask: Option<u16>, blend: LayerBlend, weight: f32 },
    /// The playing move's clip over `onto`: all of it, or by the row's mask.
    Combat { onto: u8 },
    /// An additive layer, in [`GraphProgram::additive`].
    Additive { additive: u16, onto: u8 },
    /// The playing reaction, if any.
    Reactions { onto: u8 },
}

impl PoseOp {
    /// The register it writes.
    #[must_use]
    pub fn target(self) -> u8 {
        match self {
            Self::Clip { into, .. }
            | Self::Blend { into, .. }
            | Self::Entry { into, .. }
            | Self::Machine { into, .. } => into,
            Self::Layer { onto, .. }
            | Self::Combat { onto }
            | Self::Additive { onto, .. }
            | Self::Reactions { onto } => onto,
        }
    }

    /// The registers above [`Self::target`] it may use as scratch: a sample to blend in, a catch's
    /// second pose, an additive's difference. A machine's are its states'.
    #[must_use]
    pub fn scratch(self) -> u8 {
        match self {
            Self::Clip { .. } | Self::Machine { .. } | Self::Layer { .. } => 0,
            Self::Blend { .. } | Self::Combat { .. } => 1,
            Self::Entry { .. } | Self::Additive { .. } | Self::Reactions { .. } => 2,
        }
    }
}

/// A blend space.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramBlend {
    /// A 1D blend's one parameter, or a 2D blend's two.
    pub axes: BlendAxes,
    pub samples: Span,
    pub rate: [f32; 2],
    pub sync: Option<u8>,
    pub footwork: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum BlendAxes {
    /// Linear between the two neighbours.
    One(u8),
    /// Gradient bands, polar (v1's).
    Two(u8, u8),
}

#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramSample {
    pub clip: u16,
    /// A 1D sample's place is `at[0]`.
    pub at: [f32; 2],
    pub phase: f32,
    pub sync: bool,
}

/// A takeoff or a landing: v1's entry blends.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramEntry {
    pub landing: bool,
    pub standing: u16,
    pub moving: Vec<ProgramEntrySample>,
    pub anticipate: bool,
    pub carry: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramEntrySample {
    pub clip: u16,
    pub fade: [f32; 2],
    pub short_lead: Option<[f32; 2]>,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramFootwork {
    pub brake: [u16; 2],
    pub launch: [u16; 2],
    /// A clip, and the share of its cycle a ball is down.
    pub gaits: Vec<(u16, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramCombat {
    pub decay: u16,
    pub exit: u16,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramMove {
    pub name: String,
    pub clip: u16,
    pub mask: Option<u16>,
    pub decay: u16,
    /// By [`Side`], in [`Side::ALL`]'s order.
    pub variants: [Option<u16>; 4],
}

impl ProgramMove {
    /// The clip a move started by a hit from `side` plays.
    #[must_use]
    pub fn clip_from(&self, side: Option<Side>) -> u16 {
        side.and_then(|side| self.variants[side as usize]).unwrap_or(self.clip)
    }
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramAdditive {
    pub name: String,
    pub clip: u16,
    pub mask: Option<u16>,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramReaction {
    pub name: String,
    pub clip: u16,
    pub mask: Option<u16>,
    pub weight: f32,
    pub fade_out: u16,
    pub restart: u16,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ProgramEvent {
    pub name: String,
    pub from: ProgramEventSource,
    pub min_weight: f32,
}

#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ProgramEventSource {
    Foot { foot: Foot, contact: Contact },
    Clip(String),
}

/// A program's worst case, from its compile.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GraphCost {
    /// The most clips one evaluation of the layers samples. A frame that starts a decay evaluates three
    /// times, a landing's or turn's hand-off scores gait phases with whole locomotion evaluations, and
    /// a new additive layer or reaction samples one more clip for its reference (the module's "Cost").
    /// At most [`MAX_CLIPS_SAMPLED`].
    pub clips_sampled: u16,
    /// Pose registers. At most [`MAX_REGISTERS`].
    pub registers: u8,
    pub ops: u16,
    /// The most condition steps one machine's transitions take from one state, all of them tried.
    pub condition_ops: u16,
    /// The deepest any condition's stack goes.
    pub condition_depth: u8,
    /// The deepest machine, counting the ones it plays inside.
    pub machine_depth: u8,
}

impl GraphProgram {
    /// The ops a state runs.
    #[must_use]
    pub fn state_ops(&self, state: u16) -> &[PoseOp] {
        self.states.get(usize::from(state)).map_or(&[], |state| &self.ops[state.ops.range()])
    }

    /// The ops a frame runs, bottom layer first.
    #[must_use]
    pub fn layer_ops(&self) -> &[PoseOp] {
        &self.ops[self.layers.range()]
    }

    /// A machine's transitions, in the order they're tried.
    #[must_use]
    pub fn transitions(&self, machine: u16) -> &[ProgramTransition] {
        self.machines
            .get(usize::from(machine))
            .map_or(&[], |machine| &self.transitions[machine.transitions.range()])
    }

    /// Whether `transition`'s condition holds, over value registers `values` (a missing one reads 0)
    /// and whether the state has `finished`. Allocates nothing.
    #[must_use]
    pub fn holds(&self, transition: &ProgramTransition, values: &[f32], finished: bool) -> bool {
        let value = |index: u8| values.get(usize::from(index)).copied().unwrap_or(0.0);
        let mut stack = [false; MAX_CONDITION_DEPTH];
        let mut top = 0;
        for op in &self.conditions[transition.condition.range()] {
            let pushed = match *op {
                ConditionOp::Constant(truth) => truth,
                ConditionOp::Truth(parameter) => value(parameter) != 0.0,
                ConditionOp::Compare { parameter, comparison, value: against } => {
                    comparison.holds(value(parameter), against)
                }
                ConditionOp::Between { left, comparison, right } => {
                    comparison.holds(value(left), value(right))
                }
                ConditionOp::Mode { parameter, mode } => value(parameter) == f32::from(mode),
                ConditionOp::Finished => finished,
                ConditionOp::Not => {
                    stack[top - 1] = !stack[top - 1];
                    continue;
                }
                ConditionOp::And | ConditionOp::Or => {
                    top -= 1;
                    let (a, b) = (stack[top - 1], stack[top]);
                    stack[top - 1] = if *op == ConditionOp::And { a && b } else { a || b };
                    continue;
                }
            };
            stack[top] = pushed;
            top += 1;
        }
        stack[0]
    }

    /// The row for the move named `name`.
    #[must_use]
    pub fn move_row(&self, name: &str) -> Option<&ProgramMove> {
        self.moves.binary_search_by(|row| row.name.as_str().cmp(name)).ok().map(|index| &self.moves[index])
    }

    /// A reaction's index, by name.
    #[must_use]
    pub fn reaction(&self, name: &str) -> Option<u16> {
        self.reactions.iter().position(|reaction| reaction.name == name).map(|index| index as u16)
    }

    /// A parameter's value register, by name: for binding a game's slots once.
    #[must_use]
    pub fn parameter(&self, name: &str) -> Option<u8> {
        self.parameters.iter().position(|parameter| parameter.name == name).map(|index| index as u8)
    }

    /// The clips it names, by ID, for the asset server's dependencies.
    pub fn clip_ids(&self) -> impl Iterator<Item = AssetId> + '_ {
        self.clips.iter().map(|clip| clip.id)
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes, checking the envelope, the archive and that every index and span lands in
    /// its table, so nothing a frame reads can be out of range.
    pub fn read(bytes: &[u8]) -> Result<Self, GraphError> {
        let archived = sapphire_serialize::access::<Self>(bytes)?;
        let program = rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .map_err(|error| GraphError::Damaged(error.to_string()))?;
        program.validate().map_err(|what| GraphError::Damaged(what.to_owned()))?;
        Ok(program)
    }

    /// Whether every index and span is in range, every table within the compiler's limits, every state
    /// and transition in the machine that holds it, every nested machine one depth below the machine
    /// that plays it, and every condition balanced: what no compile can get wrong, and damaged bytes can.
    /// A runtime sized by the limits can then run whatever reads.
    fn validate(&self) -> Result<(), &'static str> {
        let fits = |ok: bool, what: &'static str| if ok { Ok(()) } else { Err(what) };
        let clip = |index: u16| usize::from(index) < self.clips.len();
        let mask = |index: Option<u16>| index.is_none_or(|index| usize::from(index) < self.masks.len());
        let sync = |index: Option<u8>| index.is_none_or(|index| usize::from(index) < self.sync.len());
        let value = |index: u8| usize::from(index) < self.parameters.len();
        let registers = usize::from(self.cost.registers);
        fits(self.parameters.len() <= MAX_PARAMETERS && registers <= MAX_REGISTERS, "too many registers")?;
        let cost = &self.cost;
        fits(
            usize::from(cost.clips_sampled) <= MAX_CLIPS_SAMPLED
                && usize::from(cost.machine_depth) <= MAX_MACHINE_DEPTH
                && usize::from(cost.condition_depth) <= MAX_CONDITION_DEPTH,
            "the cost past its limits",
        )?;
        fits(
            self.clips.len() <= MAX_GRAPH_CLIPS
                && self.masks.len() <= MAX_MASKS
                && self.masks.iter().all(|mask| mask.bones.len() <= MAX_MASK_BONES)
                && self.sync.len() <= MAX_SYNC_GROUPS
                && self.machines.len() <= MAX_MACHINES
                && self.states.len() <= MAX_MACHINES * MAX_STATES
                && self.transitions.len() <= MAX_MACHINES * MAX_TRANSITIONS
                && usize::from(self.layers.len) <= MAX_LAYERS
                && self.moves.len() <= MAX_GRAPH_MOVES
                && self.reactions.len() <= MAX_REACTIONS
                && self.events.len() <= MAX_EVENTS,
            "a table past its limit",
        )?;
        fits(self.layers.fits(self.ops.len()), "the layers' ops")?;
        for (index, machine) in self.machines.iter().enumerate() {
            fits(
                machine.states.fits(self.states.len())
                    && (1..=MAX_STATES).contains(&usize::from(machine.states.len))
                    && machine.states.range().contains(&usize::from(machine.start))
                    && machine.transitions.fits(self.transitions.len())
                    && usize::from(machine.transitions.len) <= MAX_TRANSITIONS
                    && usize::from(machine.register) < registers
                    && (1..=MAX_MACHINE_DEPTH).contains(&usize::from(machine.depth)),
                "a machine",
            )?;
            // Its states and transitions are its own: a state names it, a transition leaves and enters
            // only its states.
            let own = |state: u16| machine.states.range().contains(&usize::from(state));
            fits(
                self.states[machine.states.range()].iter().all(|state| usize::from(state.machine) == index)
                    && self.transitions[machine.transitions.range()]
                        .iter()
                        .all(|transition| transition.from.is_none_or(own) && own(transition.to)),
                "a machine's states or transitions",
            )?;
        }
        for (index, state) in self.states.iter().enumerate() {
            let owner = self.machines.get(usize::from(state.machine));
            fits(
                state.ops.fits(self.ops.len())
                    && owner.is_some_and(|owner| owner.states.range().contains(&index)),
                "a state",
            )?;
            // A machine a state plays sits one depth below the state's own, so playing never recurses
            // past MAX_MACHINE_DEPTH.
            let depth = owner.map_or(0, |owner| owner.depth);
            fits(
                self.ops[state.ops.range()].iter().all(|op| match *op {
                    PoseOp::Machine { machine, .. } => self
                        .machines
                        .get(usize::from(machine))
                        .is_some_and(|nested| nested.depth == depth.saturating_add(1)),
                    _ => true,
                }),
                "a nested machine",
            )?;
        }
        fits(
            self.ops[self.layers.range()].iter().all(|op| match *op {
                PoseOp::Machine { machine, .. } => {
                    self.machines.get(usize::from(machine)).is_some_and(|played| played.depth == 1)
                }
                _ => true,
            }),
            "a layer's machine",
        )?;
        for transition in &self.transitions {
            let state = |index: u16| usize::from(index) < self.states.len();
            fits(
                transition.from.is_none_or(state)
                    && state(transition.to)
                    && transition.condition.fits(self.conditions.len())
                    && usize::from(transition.condition.len) <= MAX_CONDITION_OPS,
                "a transition",
            )?;
            let ops = &self.conditions[transition.condition.range()];
            let mut height = 0usize;
            for op in ops {
                let ok = match *op {
                    ConditionOp::Truth(parameter) | ConditionOp::Compare { parameter, .. } => {
                        value(parameter)
                    }
                    ConditionOp::Between { left, right, .. } => value(left) && value(right),
                    ConditionOp::Mode { parameter, mode } => {
                        value(parameter) && usize::from(mode) < TraversalMode::ALL.len()
                    }
                    ConditionOp::Not => height >= 1,
                    ConditionOp::And | ConditionOp::Or => height >= 2,
                    ConditionOp::Constant(_) | ConditionOp::Finished => true,
                };
                fits(ok, "a condition's step")?;
                match op {
                    ConditionOp::And | ConditionOp::Or => height -= 1,
                    ConditionOp::Not => {}
                    _ => height += 1,
                }
            }
            fits(height == 1 && depth(ops) <= MAX_CONDITION_DEPTH, "a condition's stack")?;
        }
        for op in &self.ops {
            let ok = match *op {
                PoseOp::Clip { clip: index, sync: group, .. } => clip(index) && sync(group),
                PoseOp::Blend { blend, .. } => usize::from(blend) < self.blends.len(),
                PoseOp::Entry { entry, .. } => usize::from(entry) < self.entries.len(),
                PoseOp::Machine { machine, into } => {
                    self.machines.get(usize::from(machine)).is_some_and(|machine| machine.register == into)
                }
                PoseOp::Layer { from, mask: index, .. } => usize::from(from) < registers && mask(index),
                PoseOp::Combat { .. } | PoseOp::Reactions { .. } => true,
                PoseOp::Additive { additive, .. } => usize::from(additive) < self.additive.len(),
            };
            fits(ok && usize::from(op.target()) + usize::from(op.scratch()) < registers, "an op")?;
        }
        for blend in &self.blends {
            let (axes, most) = match blend.axes {
                BlendAxes::One(axis) => (value(axis), MAX_BLEND_1D_SAMPLES),
                BlendAxes::Two(x, y) => (value(x) && value(y), MAX_BLEND_SAMPLES),
            };
            fits(
                axes && blend.samples.fits(self.samples.len())
                    && (1..=most).contains(&usize::from(blend.samples.len))
                    && sync(blend.sync)
                    && blend.footwork.is_none_or(|index| usize::from(index) < self.footwork.len()),
                "a blend",
            )?;
        }
        fits(self.samples.iter().all(|sample| clip(sample.clip)), "a blend's sample")?;
        for entry in &self.entries {
            fits(
                clip(entry.standing)
                    // As the compiler holds them: a takeoff has one moving clip, a landing up to three.
                    && (1..=if entry.landing { 3 } else { 1 }).contains(&entry.moving.len())
                    && entry.moving.iter().all(|sample| clip(sample.clip)),
                "an entry",
            )?;
        }
        for footwork in &self.footwork {
            fits(
                footwork.brake.iter().chain(&footwork.launch).all(|&index| clip(index))
                    && !footwork.gaits.is_empty()
                    && footwork.gaits.len() <= MAX_BLEND_SAMPLES
                    && footwork.gaits.iter().all(|&(index, _)| clip(index)),
                "a footwork",
            )?;
        }
        fits(
            self.moves.windows(2).all(|pair| pair[0].name < pair[1].name)
                && self.moves.iter().all(|row| {
                    clip(row.clip)
                        && mask(row.mask)
                        && row.variants.iter().all(|variant| variant.is_none_or(clip))
                }),
            "a move's row",
        )?;
        fits(
            self.additive.len() <= MAX_ADDITIVE_LAYERS
                && self.additive.iter().all(|layer| clip(layer.clip) && mask(layer.mask))
                && self.reactions.iter().all(|layer| clip(layer.clip) && mask(layer.mask)),
            "a layer's clip",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comparisons_flip_to_the_same_test() {
        for comparison in [
            Comparison::Less,
            Comparison::AtMost,
            Comparison::Greater,
            Comparison::AtLeast,
            Comparison::Equal,
            Comparison::NotEqual,
        ] {
            for (a, b) in [(1.0, 2.0), (2.0, 1.0), (1.5, 1.5)] {
                assert_eq!(
                    comparison.holds(a, b),
                    comparison.flipped().holds(b, a),
                    "{comparison:?} {a} {b}"
                );
            }
        }
    }
}
