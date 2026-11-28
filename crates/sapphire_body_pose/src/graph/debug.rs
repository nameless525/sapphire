//! What a graph is doing, for a person to see, and the parameters they set by hand (M6 A9): the trace the
//! editor's graph pane reads over live link (T17b), and its overrides.
//!
//! [`GraphPlayer::debug_snapshot`] writes into a [`GraphSnapshot`] the caller owns and keeps: each machine
//! layer's active state and how long it has played, each layer's blend weights, every parameter's value, the
//! controller's inputs the graph doesn't name as parameters, and the move playing. It is fixed arrays of plain values, so taking one allocates nothing, ever, and it is at
//! most [`GraphSnapshot::MAX_BYTES`].
//!
//! # Overrides are presentation
//!
//! [`GraphPlayer::set_override`] pins a parameter to a value until [`GraphPlayer::clear_overrides`]. The
//! controller's inputs (`speed`, `direction`, `grounded`, `landed`, `preparing_jump`) are overridden in the
//! input, so all of the graph sees a controller doing that: the smoothed velocity follows an overridden
//! speed, and a blend over it and a condition on `smoothed_speed` with it. They can be overridden in every
//! graph, since every graph is fed them, named as parameters or not: a converted graph names only what its
//! conditions and blends read (the heroine's reads `smoothed_speed` and the smoothed velocity, never
//! `speed`). Any other parameter (a smoothed value, `forward`, a game slot) is overridden in its register,
//! after the registers are read, and only in a graph that names it. A velocity blend and the gait's phase
//! rate read the smoothed velocity itself, so overriding `velocity_x`, `velocity_z` or `smoothed_speed`
//! moves a condition on them but not the blend or the phase: override `speed` and `direction` to move them.
//! On a frozen clock the smoothed velocity takes an overridden `speed` at once, having no time to smooth
//! over, so a held frame's blends show it; a change of state an override brings keeps the pose drawn until
//! the clock runs. No checksum can move,
//! because of where a player that takes one lives: the fixed step's player is inside its
//! [`FixedGraphHistory`](super::FixedGraphHistory), which hands out no `&mut GraphPlayer` and has no
//! override of its own, so only a presentation player can hold one, and presentation writes nothing the
//! simulation reads. What hashes the fixed step's graph (`FixedGraphHistory::write_canonical`) writes its
//! player's input, never its overrides. A character whose pose comes from the fixed step (a `FixedBody`) has
//! every parameter read by the fixed step, so whoever routes an override to it refuses it; its presentation
//! player plays nothing.

use sapphire_assets::graph::MAX_BLEND_SAMPLES;
use sapphire_assets::graph::v2::{LocomotionValue, MAX_LAYERS, MAX_PARAMETERS};
use sapphire_core::Name;

use super::{GraphBinding, GraphInput, GraphPlayer, LayerOp, Motion, Value};

/// One machine layer, as a snapshot has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerTrace {
    /// The machine of the state playing, which is its layer's or one nested in it.
    pub machine: Name,
    /// The state playing, entered to its leaf.
    pub state: Name,
    /// How long it has played, in the owner clock's ticks, at the frame drawn.
    pub time: f64,
    weights: [f32; MAX_BLEND_SAMPLES],
    weight_count: u8,
}

impl LayerTrace {
    const EMPTY: Self = Self {
        machine: Name::new(""),
        state: Name::new(""),
        time: 0.0,
        weights: [0.0; MAX_BLEND_SAMPLES],
        weight_count: 0,
    };

    /// The weight of each of the state's blend samples, in the document's order: empty for a state
    /// that isn't a blend.
    #[must_use]
    pub fn weights(&self) -> &[f32] {
        &self.weights[..usize::from(self.weight_count)]
    }
}

/// A parameter, as a snapshot has it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParameterTrace {
    pub name: Name,
    /// What the graph read, overridden or not.
    pub value: f32,
    pub overridden: bool,
}

impl ParameterTrace {
    const EMPTY: Self = Self { name: Name::new(""), value: 0.0, overridden: false };
}

/// The controller's inputs, by the names version 1's converter gives them, which every graph is fed. They
/// are found by name: a graph naming a parameter after one gives that name to its parameter, so the input
/// of that name is then traced and overridden only through it, and one read under another name is listed
/// under both.
const INPUTS: [(Name, &str, LocomotionValue); 5] = [
    (Name::new("speed"), "speed", LocomotionValue::Speed),
    (Name::new("direction"), "direction", LocomotionValue::Direction),
    (Name::new("grounded"), "grounded", LocomotionValue::Grounded),
    (Name::new("landed"), "landed", LocomotionValue::Landed),
    (Name::new("preparing_jump"), "preparing_jump", LocomotionValue::PreparingJump),
];

/// Each of `texts` by its name, once, for [`GraphPlayer::label`].
pub(super) fn labels<'a>(texts: impl IntoIterator<Item = &'a str>) -> Vec<(Name, Box<str>)> {
    let mut labels: Vec<(Name, Box<str>)> =
        texts.into_iter().map(|text| (Name::intern(text), text.into())).collect();
    labels.sort_unstable_by_key(|&(name, _)| name);
    labels.dedup_by_key(|&mut (name, _)| name);
    labels
}

/// The most names a graph can be overridden by and a snapshot lists: the graph's parameters, then the
/// controller's inputs it doesn't name.
const MAX_TRACED: usize = MAX_PARAMETERS + INPUTS.len();

/// What `name` reads in `graph`: its parameter's source, or the controller's input of that name the graph
/// doesn't name.
fn read_of(graph: &GraphBinding, name: Name) -> Option<Value> {
    match graph.parameter_names.iter().position(|&found| found == name) {
        Some(register) => graph.parameters.get(register).copied(),
        None => INPUTS.iter().find(|&&(input, ..)| input == name).map(|&(.., read)| Value::Locomotion(read)),
    }
}

/// The controller's `read` input in `input`, as a number.
fn input_value(input: &GraphInput, read: LocomotionValue) -> f32 {
    let locomotion = &input.locomotion;
    match read {
        LocomotionValue::Speed => locomotion.speed,
        LocomotionValue::Direction => locomotion.direction,
        LocomotionValue::Grounded => f32::from(u8::from(locomotion.grounded)),
        LocomotionValue::Landed => f32::from(u8::from(locomotion.landed)),
        LocomotionValue::PreparingJump => f32::from(u8::from(locomotion.preparing_jump)),
        LocomotionValue::Forward
        | LocomotionValue::SmoothedSpeed
        | LocomotionValue::SmoothedX
        | LocomotionValue::SmoothedZ => 0.0,
    }
}

/// What a graph is doing: [`GraphPlayer::debug_snapshot`] writes it. Make one, keep it and take a snapshot
/// into it as often as wanted: it's plain values, at most [`Self::MAX_BYTES`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphSnapshot {
    /// When the pose was last drawn, on the owner's clock: `None` before the first frame.
    pub drawn: Option<f64>,
    /// How many replacements the player has adopted ([`GraphPlayer::replace`]).
    pub revision: u32,
    /// Whether a transition's offset is still decaying out.
    pub decaying: bool,
    /// The move playing, by name, and how far into it, in ticks.
    pub combat: Option<(Name, f64)>,
    layers: [LayerTrace; MAX_LAYERS],
    layer_count: u8,
    parameters: [ParameterTrace; MAX_TRACED],
    parameter_count: u8,
}

impl Default for GraphSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphSnapshot {
    /// The most a snapshot takes, wherever it is kept: the size cap T17b's pane and live link plan on.
    pub const MAX_BYTES: usize = 2048;

    /// An empty one, for a player to write into.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            drawn: None,
            revision: 0,
            decaying: false,
            combat: None,
            layers: [LayerTrace::EMPTY; MAX_LAYERS],
            layer_count: 0,
            parameters: [ParameterTrace::EMPTY; MAX_TRACED],
            parameter_count: 0,
        }
    }

    /// Each machine layer, bottom first: the base's machine, then each layer that plays one.
    #[must_use]
    pub fn layers(&self) -> &[LayerTrace] {
        &self.layers[..usize::from(self.layer_count)]
    }

    /// Every parameter, in the program's register order, then each of the controller's inputs (`speed`,
    /// `direction`, `grounded`, `landed`, `preparing_jump`) the graph doesn't name, as the graph was fed it.
    #[must_use]
    pub fn parameters(&self) -> &[ParameterTrace] {
        &self.parameters[..usize::from(self.parameter_count)]
    }

    /// The parameter or controller input of this name, if the snapshot has it.
    #[must_use]
    pub fn parameter(&self, name: Name) -> Option<&ParameterTrace> {
        self.parameters().iter().find(|parameter| parameter.name == name)
    }
}

const _: () = assert!(size_of::<GraphSnapshot>() <= GraphSnapshot::MAX_BYTES);

/// Why an override wasn't taken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum OverrideError {
    #[error("the graph has no parameter `{0}`, and the controller no input of that name")]
    NoSuchParameter(Name),
    #[error("an override is a number")]
    NotANumber,
    #[error("{MAX_TRACED} overrides at most, every one a name this graph has")]
    Full,
}

/// The parameters set by hand, by name, so they find their registers in a replacement graph too: one its
/// graph doesn't have is held and does nothing until a graph that has it comes, or gives way to one the
/// graph has once the table is full. It holds as many names as a graph can be overridden by.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Overrides {
    names: [Name; MAX_TRACED],
    values: [f32; MAX_TRACED],
    len: u8,
}

impl Overrides {
    pub(super) const NONE: Self =
        Self { names: [Name::new(""); MAX_TRACED], values: [0.0; MAX_TRACED], len: 0 };

    fn iter(&self) -> impl Iterator<Item = (Name, f32)> + '_ {
        self.names.iter().copied().zip(self.values.iter().copied()).take(usize::from(self.len))
    }

    fn of(&self, name: Name) -> Option<f32> {
        self.iter().find(|&(overridden, _)| overridden == name).map(|(_, value)| value)
    }

    /// The parameter `register` of `graph` reads, if it's overridden.
    fn of_register(&self, graph: &GraphBinding, register: usize) -> Option<f32> {
        self.of(*graph.parameter_names.get(register)?)
    }

    /// `input` with the controller's inputs overridden, by the parameter that reads one or by the input's
    /// own name.
    pub(super) fn onto_input(&self, graph: &GraphBinding, mut input: GraphInput) -> GraphInput {
        if self.len == 0 {
            return input;
        }
        let locomotion = &mut input.locomotion;
        for (name, overridden) in self.iter() {
            let Some(Value::Locomotion(read)) = read_of(graph, name) else { continue };
            match read {
                LocomotionValue::Speed => locomotion.speed = overridden,
                LocomotionValue::Direction => locomotion.direction = overridden,
                LocomotionValue::Grounded => locomotion.grounded = overridden != 0.0,
                LocomotionValue::Landed => locomotion.landed = overridden != 0.0,
                LocomotionValue::PreparingJump => locomotion.preparing_jump = overridden != 0.0,
                LocomotionValue::Forward
                | LocomotionValue::SmoothedSpeed
                | LocomotionValue::SmoothedX
                | LocomotionValue::SmoothedZ => {}
            }
        }
        input
    }

    /// The registers, every overridden one replaced: what [`Self::onto_input`] overrode reads the same.
    pub(super) fn onto_registers(&self, graph: &GraphBinding, registers: &mut [f32; MAX_PARAMETERS]) {
        if self.len == 0 {
            return;
        }
        for (index, register) in registers.iter_mut().enumerate().take(graph.parameters.len()) {
            if let Some(value) = self.of_register(graph, index) {
                *register = value;
            }
        }
    }
}

impl GraphPlayer {
    /// Writes what the graph is doing as of the last frame into `out`, which the caller keeps: each machine
    /// layer's state, time and blend weights, every parameter's value, and the move playing. Allocates
    /// nothing.
    pub fn debug_snapshot(&self, out: &mut GraphSnapshot) {
        let graph = &*self.graph;
        let layers = &self.layers;
        let now = self.drawn.unwrap_or(0.0);
        out.drawn = self.drawn;
        out.revision = self.revision;
        out.decaying = self.is_decaying();
        out.combat = layers
            .combat
            .map(|playing| (graph.moves[usize::from(playing.entry)].name, playing.position.max(0.0)));
        out.layer_count = 0;
        for op in &graph.ops {
            let (state, started) = match *op {
                LayerOp::Base => (layers.state, layers.started),
                LayerOp::Machine(layer) => {
                    let active = layers.layer_states[usize::from(layer.slot)];
                    (active.state, active.started)
                }
                LayerOp::Combat | LayerOp::Additive(_) | LayerOp::Reactions => continue,
            };
            let Some(trace) = out.layers.get_mut(usize::from(out.layer_count)) else { break };
            let bound = &graph.states[usize::from(state)];
            trace.machine = graph.machines[usize::from(bound.machine)].name;
            trace.state = bound.name;
            trace.time = (now - started).max(0.0);
            trace.weights = [0.0; MAX_BLEND_SAMPLES];
            trace.weight_count = 0;
            if let Motion::Blend(space) = &bound.motion {
                if matches!(op, LayerOp::Base) {
                    trace.weights = layers.blend;
                } else {
                    layers.blend_weights(graph, state, &mut trace.weights);
                }
                trace.weight_count = space.samples.len().min(MAX_BLEND_SAMPLES) as u8;
            }
            out.layer_count += 1;
        }
        out.parameter_count = 0;
        let named = graph.parameter_names.iter().zip(&layers.values).map(|(&name, &value)| (name, value));
        let fed = self.key.map(|key| key.input).unwrap_or_default();
        let inputs = INPUTS
            .iter()
            .filter(|&&(name, ..)| !graph.parameter_names.contains(&name))
            .map(|&(name, _, read)| (name, input_value(&fed, read)));
        for (trace, (name, value)) in out.parameters.iter_mut().zip(named.chain(inputs)) {
            *trace = ParameterTrace { name, value, overridden: self.overrides.of(name).is_some() };
            out.parameter_count += 1;
        }
    }

    /// Pins the graph's parameter or the controller's input `name` to `value` from the next frame on, in
    /// place of what it reads, until [`Self::clear_overrides`]: a person's live tuning, never the game's
    /// (that's [`Self::set_game_slot`]). A truth (`grounded`, `landed`, `preparing_jump`) is 1 for anything
    /// but 0. It holds through a replacement graph, by name. Presentation only: the fixed step's player,
    /// inside its [`FixedGraphHistory`](super::FixedGraphHistory), can't take one, and its canonical bytes
    /// never hold one. Allocates nothing.
    pub fn set_override(&mut self, name: Name, value: f32) -> Result<(), OverrideError> {
        let value = self.override_value(name, value)?;
        let graph = &self.graph;
        let overrides = &mut self.overrides;
        let len = usize::from(overrides.len);
        let held = &overrides.names[..len];
        // A new name goes at the end, or, the table full, where a name this graph doesn't have was held
        // (from an earlier graph). Every name this graph has fits: there are `MAX_TRACED` at most.
        let at = held
            .iter()
            .position(|&found| found == name)
            .or((len < MAX_TRACED).then_some(len))
            .or_else(|| held.iter().position(|&found| read_of(graph, found).is_none()))
            .ok_or(OverrideError::Full)?;
        overrides.names[at] = name;
        overrides.values[at] = value;
        overrides.len = overrides.len.max(at as u8 + 1);
        Ok(())
    }

    /// Whether [`Self::set_override`] would take `name` at `value`, changing nothing: for a caller pinning
    /// several players, which checks them all first.
    pub fn check_override(&self, name: Name, value: f32) -> Result<(), OverrideError> {
        self.override_value(name, value).map(drop)
    }

    /// `value` as `name` holds it (a truth 1 or 0), or why it can't.
    fn override_value(&self, name: Name, value: f32) -> Result<f32, OverrideError> {
        if !value.is_finite() {
            return Err(OverrideError::NotANumber);
        }
        let read = read_of(&self.graph, name).ok_or(OverrideError::NoSuchParameter(name))?;
        use LocomotionValue as L;
        Ok(match read {
            Value::Locomotion(L::Grounded | L::Landed | L::PreparingJump) => {
                f32::from(u8::from(value != 0.0))
            }
            _ => value,
        })
    }

    /// The text of a name a snapshot of this player shows (a machine, a state, a move, a parameter or one of
    /// the controller's inputs), in any build: [`Name`] prints as text only in a debug one. For live link and
    /// tools, not a frame's work: it searches.
    #[must_use]
    pub fn label(&self, name: Name) -> Option<&str> {
        let labels = &self.graph.labels;
        labels
            .binary_search_by_key(&name, |&(found, _)| found)
            .ok()
            .map(|at| &*labels[at].1)
            .or_else(|| INPUTS.iter().find(|&&(input, ..)| input == name).map(|&(_, text, _)| text))
    }

    /// Lets every parameter read what it reads again, from the next frame on.
    pub fn clear_overrides(&mut self) {
        self.overrides = Overrides::NONE;
    }

    /// The parameters overridden, by name, and their values.
    pub fn overrides(&self) -> impl Iterator<Item = (Name, f32)> + '_ {
        self.overrides.iter()
    }
}
