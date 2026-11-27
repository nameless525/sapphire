//! The compiler: a [document](super::AnimGraph) to a [`GraphProgram`], at cook and on live link. It
//! names everything wrong at once, each by its path in the document
//! (`machines["locomotion"].transitions[3].when`), the way a problems pane points at a field. What
//! depends on a character (a clip for another skeleton, a bone its rig lacks) is the binding's to say.

use std::collections::BTreeMap;
use std::fmt;

use super::condition;
use super::program::{
    BlendAxes, GraphCost, GraphProgram, MAX_CLIPS_SAMPLED, MAX_REGISTERS, PoseOp, ProgramAdditive,
    ProgramBlend, ProgramClip, ProgramCombat, ProgramEntry, ProgramEntrySample, ProgramEvent,
    ProgramEventSource, ProgramFootwork, ProgramMachine, ProgramMove, ProgramParameter, ProgramReaction,
    ProgramSample, ProgramState, ProgramSync, ProgramTransition, Span, Target,
};
use super::{
    ANY_STATE, AnimGraph, Blend1D, Blend2D, EntrySample, EventSource, LayerBlend, LayerMotion,
    MAX_ADDITIVE_LAYERS, MAX_BLEND_1D_SAMPLES, MAX_EVENTS, MAX_GAME_SLOTS, MAX_LAYERS, MAX_MACHINE_DEPTH,
    MAX_MACHINES, MAX_PARAMETERS, MAX_STATES, MAX_SYNC_GROUPS, MAX_TRANSITIONS, Machine, Motion,
    ParameterKind, ParameterSource,
};
use crate::clip::MAX_EVENT_NAME;
use crate::graph::{
    Blink, MAX_BLEND_SAMPLES, MAX_DECAY_TICKS, MAX_GRAPH_CLIPS, MAX_GRAPH_MOVES, MAX_MASK_BONES, MAX_MASKS,
    MAX_REACTIONS,
};

/// One thing wrong with a document, by where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphIssue {
    /// Where in the document: `machines["locomotion"].states["ground"].plays.samples[2].clip`. A named
    /// entry is named, an unnamed one counted from 0.
    pub path: String,
    pub message: String,
}

impl fmt::Display for GraphIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}`: {}", self.path, self.message)
    }
}

/// A document that didn't compile: everything wrong with it, in document order.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{}", issues.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))]
pub struct CompileError {
    pub issues: Vec<GraphIssue>,
}

/// `list["name"]`: a named entry's path.
pub(crate) fn named(list: &str, name: &str) -> String {
    format!("{list}[\"{name}\"]")
}

/// `list[index]`: an unnamed entry's path.
fn indexed(list: &str, index: usize) -> String {
    format!("{list}[{index}]")
}

#[derive(Default)]
struct Issues(Vec<GraphIssue>);

impl Issues {
    fn push(&mut self, path: impl Into<String>, message: impl Into<String>) {
        self.0.push(GraphIssue { path: path.into(), message: message.into() });
    }

    fn limit(&mut self, path: &str, count: usize, limit: usize) {
        if count > limit {
            self.push(path, format!("{count} of them, past the limit of {limit}"));
        }
    }

    /// `names` have no repeats and none is empty; returns each name's index.
    fn names<'a>(&mut self, list: &str, names: impl Iterator<Item = &'a str>) -> BTreeMap<&'a str, u16> {
        let mut seen = BTreeMap::new();
        for (index, name) in names.enumerate() {
            if name.is_empty() {
                self.push(indexed(list, index), "an empty name");
            } else if seen.insert(name, index as u16).is_some() {
                self.push(named(list, name), "this name is used twice");
            }
        }
        seen
    }

    fn weight(&mut self, path: String, value: f32) {
        if !(0.0..=1.0).contains(&value) {
            self.push(path, format!("a weight of {value} isn't 0 to 1"));
        }
    }

    fn decay(&mut self, path: String, ticks: u32) {
        if ticks > MAX_DECAY_TICKS {
            self.push(path, format!("a decay of {ticks} ticks, past {MAX_DECAY_TICKS}"));
        }
    }

    fn speeds(&mut self, path: String, [low, high]: [f32; 2]) {
        if !(low.is_finite() && high.is_finite() && low >= 0.0 && high > low) {
            self.push(path, format!("({low}, {high}) isn't an increasing range of speeds"));
        }
    }
}

/// The document's names, each to its index.
struct Names<'a> {
    clips: BTreeMap<&'a str, u16>,
    masks: BTreeMap<&'a str, u16>,
    sync: BTreeMap<&'a str, u16>,
    machines: BTreeMap<&'a str, u16>,
    parameters: Vec<(&'a str, ParameterKind)>,
}

impl Names<'_> {
    fn clip(&self, issues: &mut Issues, path: String, name: &str) -> u16 {
        self.clips.get(name).copied().unwrap_or_else(|| {
            issues.push(path, format!("plays `{name}`, which `clips` doesn't name"));
            0
        })
    }

    fn mask(&self, issues: &mut Issues, path: String, name: Option<&String>) -> Option<u16> {
        let name = name?;
        let found = self.masks.get(name.as_str()).copied();
        if found.is_none() {
            issues.push(path, format!("uses mask `{name}`, which `masks` doesn't name"));
        }
        found
    }

    fn sync(&self, issues: &mut Issues, path: String, name: Option<&String>) -> Option<u8> {
        let name = name?;
        let found = self.sync.get(name.as_str()).map(|&index| index as u8);
        if found.is_none() {
            issues.push(path, format!("joins sync group `{name}`, which `sync` doesn't name"));
        }
        found
    }

    fn number(&self, issues: &mut Issues, path: String, name: &str) -> u8 {
        match self.parameters.iter().position(|(declared, _)| *declared == name) {
            Some(index) if self.parameters[index].1 == ParameterKind::Number => index as u8,
            Some(_) => {
                issues.push(path, format!("`{name}` isn't a number, so it can't place a blend's samples"));
                0
            }
            None => {
                issues.push(path, format!("reads `{name}`, which `parameters` doesn't name"));
                0
            }
        }
    }
}

/// Where each machine plays, found walking the layers and then the states.
struct Placement {
    register: u8,
    depth: u8,
}

/// Compiles `graph` for presentation, or names everything wrong with it.
pub fn compile(graph: &AnimGraph) -> Result<GraphProgram, CompileError> {
    compile_for(graph, Target::Presentation)
}

/// Compiles `graph` to run where `target` says, or names everything wrong with it: for
/// [`Target::Simulation`], every node only presentation may run too.
pub fn compile_for(graph: &AnimGraph, target: Target) -> Result<GraphProgram, CompileError> {
    let mut issues = Issues::default();
    let parameters = check_parameters(graph, target, &mut issues);
    if !(graph.smoothing.is_finite() && graph.smoothing >= 0.0) {
        issues.push("smoothing", format!("{} isn't a half-life", graph.smoothing));
    }
    issues.limit("clips", graph.clips.len(), MAX_GRAPH_CLIPS);
    let clips = issues.names("clips", graph.clips.iter().map(|clip| clip.name.as_str()));
    issues.limit("masks", graph.masks.len(), MAX_MASKS);
    let masks = issues.names("masks", graph.masks.iter().map(|mask| mask.name.as_str()));
    for mask in &graph.masks {
        let at = named("masks", &mask.name);
        issues.limit(&format!("{at}.bones"), mask.bones.len(), MAX_MASK_BONES);
        for (index, bone) in mask.bones.iter().enumerate() {
            issues.weight(format!("{}.weight", indexed(&format!("{at}.bones"), index)), bone.weight);
        }
    }
    issues.limit("sync", graph.sync.len(), MAX_SYNC_GROUPS);
    let sync = issues.names("sync", graph.sync.iter().map(|group| group.name.as_str()));
    issues.limit("machines", graph.machines.len(), MAX_MACHINES);
    let machines = issues.names("machines", graph.machines.iter().map(|machine| machine.name.as_str()));
    let names = Names { clips, masks, sync, machines, parameters };
    let placements = place_machines(graph, &names, &mut issues);

    let mut program = GraphProgram {
        parameters: graph
            .parameters
            .iter()
            .map(|parameter| ProgramParameter { name: parameter.name.clone(), from: parameter.from.clone() })
            .collect(),
        smoothing: graph.smoothing,
        clips: graph.clips.iter().map(|clip| ProgramClip { name: clip.name.clone(), id: clip.id }).collect(),
        masks: graph.masks.clone(),
        sync: graph
            .sync
            .iter()
            .map(|group| ProgramSync { name: group.name.clone(), foot: group.foot, contact: group.contact })
            .collect(),
        machines: Vec::new(),
        states: Vec::new(),
        transitions: Vec::new(),
        conditions: Vec::new(),
        ops: Vec::new(),
        layers: Span::default(),
        blends: Vec::new(),
        samples: Vec::new(),
        entries: Vec::new(),
        footwork: Vec::new(),
        combat: None,
        moves: Vec::new(),
        additive: Vec::new(),
        reactions: Vec::new(),
        events: Vec::new(),
        blink: graph.blink.clone(),
        cost: GraphCost::default(),
        target,
    };
    // Every machine's states first, so a transition can name a state of its machine by index.
    let mut first_state = Vec::with_capacity(graph.machines.len());
    for (index, machine) in graph.machines.iter().enumerate() {
        first_state.push(program.states.len() as u16);
        for state in &machine.states {
            program.states.push(ProgramState {
                name: state.name.clone(),
                machine: index as u16,
                ops: Span::default(),
                finishes: finishes(&state.plays),
            });
        }
    }
    let mut state_cost = vec![0u16; program.states.len()];
    let mut condition_ops = 0u16;
    let mut condition_depth = 0u8;
    for (index, machine) in graph.machines.iter().enumerate() {
        let placement = placements.get(&index).unwrap_or(&Placement { register: 0, depth: 1 });
        let first = first_state[index];
        let at = named("machines", &machine.name);
        let states = compile_machine_states(
            machine,
            &at,
            first,
            placement.register,
            &names,
            &mut program,
            &mut state_cost,
            &mut issues,
        );
        let transitions = program.transitions.len() as u16;
        let (ops, depth) =
            compile_transitions(machine, &at, first, &states, &names, &mut program, &mut issues);
        condition_ops = condition_ops.max(ops);
        condition_depth = condition_depth.max(depth);
        program.machines.push(ProgramMachine {
            name: machine.name.clone(),
            start: states.get(machine.start.as_str()).map_or(first, |&state| first + state),
            states: Span { start: first, len: machine.states.len() as u16 },
            transitions: Span { start: transitions, len: program.transitions.len() as u16 - transitions },
            register: placement.register,
            depth: placement.depth,
        });
    }
    // A machine samples its costliest state's clips; a state playing a machine, that machine's. Machines
    // nest at most `MAX_MACHINE_DEPTH` deep, so as many passes settle it.
    for _ in 0..MAX_MACHINE_DEPTH {
        for (index, machine) in graph.machines.iter().enumerate() {
            for (offset, state) in machine.states.iter().enumerate() {
                if let Motion::Machine(name) = &state.plays
                    && let Some(&inner) = names.machines.get(name.as_str())
                {
                    state_cost[usize::from(first_state[index]) + offset] =
                        machine_cost(&program, &state_cost, inner);
                }
            }
        }
    }
    let layers_start = program.ops.len() as u16;
    let clips_sampled =
        compile_layers(graph, target, &names, &placements, &mut program, &state_cost, &mut issues);
    program.layers = Span { start: layers_start, len: program.ops.len() as u16 - layers_start };
    compile_events(graph, target, &mut program, &mut issues);
    if let Some(blink) = &graph.blink {
        check_blink(blink, &mut issues);
    }

    let registers = program
        .ops
        .iter()
        .map(|op| usize::from(op.target()) + usize::from(op.scratch()) + 1)
        .max()
        .unwrap_or(1);
    if registers > MAX_REGISTERS {
        issues.push("layers", format!("needs {registers} pose registers, past the limit of {MAX_REGISTERS}"));
    }
    if usize::from(clips_sampled) > MAX_CLIPS_SAMPLED {
        let message = format!(
            "samples up to {clips_sampled} clips an evaluation of the layers, past the limit of {MAX_CLIPS_SAMPLED}"
        );
        issues.push("layers", message);
    }
    program.cost = GraphCost {
        clips_sampled,
        registers: registers.min(usize::from(u8::MAX)) as u8,
        ops: program.ops.len() as u16,
        condition_ops,
        condition_depth,
        machine_depth: placements.values().map(|placement| placement.depth).max().unwrap_or(0),
    };
    if issues.0.is_empty() { Ok(program) } else { Err(CompileError { issues: issues.0 }) }
}

fn check_parameters<'a>(
    graph: &'a AnimGraph,
    target: Target,
    issues: &mut Issues,
) -> Vec<(&'a str, ParameterKind)> {
    issues.limit("parameters", graph.parameters.len(), MAX_PARAMETERS);
    issues.names("parameters", graph.parameters.iter().map(|parameter| parameter.name.as_str()));
    let mut sources: Vec<&ParameterSource> = Vec::new();
    let mut slots = 0;
    for parameter in &graph.parameters {
        let at = named("parameters", &parameter.name);
        let name = parameter.name.as_str();
        let word = name.starts_with(|c: char| c.is_ascii_lowercase())
            && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !name.is_empty() && !word {
            issues.push(&at, "a name is a lower-case letter, then lower-case letters, digits and `_`");
        }
        if ["true", "false", "finished", "finished_at"].contains(&name) {
            issues.push(&at, "this name is a word of the condition grammar");
        }
        if let ParameterSource::Game(slot) = &parameter.from {
            slots += 1;
            if slot.is_empty() {
                issues.push(format!("{at}.from"), "a game slot with no name");
            }
            if target == Target::Simulation {
                issues.push(
                    format!("{at}.from"),
                    "a game slot is presentation's to fill, and the fixed step reads nothing it wrote",
                );
            }
        }
        // Nothing binds traversal's mode until A14 adds the modes and A15 and A16 play them: it reads
        // none, so a condition on it would be dead in the checksum's step.
        if target == Target::Simulation && matches!(parameter.from, ParameterSource::Traversal(_)) {
            issues.push(format!("{at}.from"), "traversal's mode isn't bound for the fixed step yet");
        }
        if sources.contains(&&parameter.from) {
            issues.push(format!("{at}.from"), "another parameter is bound to this already");
        } else {
            sources.push(&parameter.from);
        }
    }
    if slots > MAX_GAME_SLOTS {
        issues.push("parameters", format!("{slots} game slots, past the limit of {MAX_GAME_SLOTS}"));
    }
    graph.parameters.iter().map(|parameter| (parameter.name.as_str(), parameter.from.kind())).collect()
}

/// Gives each machine the register it writes and its depth, walking down from the layers, and names a
/// machine that plays in no place, or in more than one, or inside itself.
fn place_machines(graph: &AnimGraph, names: &Names<'_>, issues: &mut Issues) -> BTreeMap<usize, Placement> {
    let mut placements = BTreeMap::new();
    let mut players: Vec<Vec<String>> = vec![Vec::new(); graph.machines.len()];
    // Which machines' states play each machine.
    let mut owners: Vec<Vec<usize>> = vec![Vec::new(); graph.machines.len()];
    let mut queue = Vec::new();
    for (index, layer) in graph.layers.iter().enumerate() {
        if let LayerMotion::Machine(played) = &layer.plays {
            let path = format!("{}.plays.machine", named("layers", &layer.name));
            match names.machines.get(played.machine.as_str()) {
                Some(&machine) => {
                    players[usize::from(machine)].push(path);
                    // The first layer writes the output; each other machine layer its own, then lays it on.
                    let into = u8::from(index > 0);
                    queue.push((usize::from(machine), Placement { register: into, depth: 1 }));
                }
                None => issues
                    .push(path, format!("plays machine `{}`, which `machines` doesn't name", played.machine)),
            }
        }
    }
    for (index, machine) in graph.machines.iter().enumerate() {
        for state in &machine.states {
            if let Motion::Machine(name) = &state.plays
                && let Some(&inner) = names.machines.get(name.as_str())
            {
                let path = format!(
                    "{}.plays",
                    named(&format!("{}.states", named("machines", &machine.name)), &state.name)
                );
                players[usize::from(inner)].push(path);
                owners[usize::from(inner)].push(index);
                if usize::from(inner) == index {
                    issues.push(named("machines", &machine.name), "plays itself");
                }
            }
        }
    }
    while let Some((machine, placement)) = queue.pop() {
        if placements.contains_key(&machine) {
            continue;
        }
        let at = named("machines", &graph.machines[machine].name);
        if usize::from(placement.depth) > MAX_MACHINE_DEPTH {
            issues.push(
                &at,
                format!("plays {} machines deep, past the limit of {MAX_MACHINE_DEPTH}", placement.depth),
            );
        }
        for state in &graph.machines[machine].states {
            if let Motion::Machine(name) = &state.plays
                && let Some(&inner) = names.machines.get(name.as_str())
            {
                queue.push((
                    usize::from(inner),
                    Placement { register: placement.register, depth: placement.depth.saturating_add(1) },
                ));
            }
        }
        placements.insert(machine, placement);
    }
    for (machine, played) in graph.machines.iter().zip(&players) {
        let at = named("machines", &machine.name);
        match played.len() {
            0 => issues.push(at, "plays nowhere: no layer or state names it"),
            1 => {}
            _ => issues.push(
                at,
                format!("plays in {} places ({}): a machine plays in one", played.len(), played.join(", ")),
            ),
        }
    }
    for (index, machine) in graph.machines.iter().enumerate() {
        if !players[index].is_empty() && !placements.contains_key(&index) {
            let message = if owners[index].iter().all(|&owner| owner == index) {
                "plays only inside itself, through its own states"
            } else {
                "plays only inside machines no layer reaches"
            };
            issues.push(named("machines", &machine.name), message);
        }
    }
    placements
}

/// Whether what a state plays can end.
fn finishes(motion: &Motion) -> bool {
    match motion {
        Motion::Clip { looping, .. } => !looping,
        Motion::Takeoff(_) | Motion::Landing(_) => true,
        Motion::Blend1D(_) | Motion::Blend2D(_) | Motion::Machine(_) => false,
    }
}

/// The most clips any of a machine's states samples.
fn machine_cost(program: &GraphProgram, state_cost: &[u16], machine: u16) -> u16 {
    let states: Vec<u16> = program
        .states
        .iter()
        .enumerate()
        .filter(|(_, state)| state.machine == machine)
        .map(|(i, _)| state_cost[i])
        .collect();
    states.into_iter().max().unwrap_or(0)
}

/// Each state's op, and the names of the machine's states.
#[expect(clippy::too_many_arguments, reason = "one machine's place, its tables and its issues")]
fn compile_machine_states<'a>(
    machine: &'a Machine,
    at: &str,
    first: u16,
    into: u8,
    names: &Names<'_>,
    program: &mut GraphProgram,
    state_cost: &mut [u16],
    issues: &mut Issues,
) -> BTreeMap<&'a str, u16> {
    let list = format!("{at}.states");
    if machine.states.is_empty() {
        issues.push(&list, "a machine of no states");
    }
    issues.limit(&list, machine.states.len(), MAX_STATES);
    let states = issues.names(&list, machine.states.iter().map(|state| state.name.as_str()));
    if states.contains_key(ANY_STATE) {
        issues.push(named(&list, ANY_STATE), "`*` means any state, so no state is named it");
    }
    if !states.contains_key(machine.start.as_str()) {
        issues.push(format!("{at}.start"), format!("`{}`: no such state", machine.start));
    }
    for (offset, state) in machine.states.iter().enumerate() {
        let path = format!("{}.plays", named(&list, &state.name));
        let index = usize::from(first) + offset;
        let (op, cost) = match &state.plays {
            Motion::Clip { clip, looping, sync } => {
                let clip = names.clip(issues, path.clone(), clip);
                let sync = names.sync(issues, format!("{path}.sync"), sync.as_ref());
                (PoseOp::Clip { clip, looping: *looping, sync, into }, 1)
            }
            Motion::Blend1D(blend) => blend_1d(blend, &path, into, names, program, issues),
            Motion::Blend2D(blend) => blend_2d(blend, &path, into, names, program, issues),
            Motion::Takeoff(takeoff) => {
                // A seek in the air starts the base at the graph's one takeoff, which reads the body's
                // flight.
                if into != 0 {
                    issues.push(&path, "a takeoff plays only in the first layer's machines");
                }
                if program.entries.iter().any(|entry| !entry.landing) {
                    issues.push(&path, "a second takeoff: a graph has one");
                }
                let entry = entry(&takeoff.standing, &takeoff.moving, 1, &path, names, issues);
                let cost = 1 + entry.len() as u16;
                program.entries.push(ProgramEntry {
                    landing: false,
                    standing: names.clips.get(takeoff.standing.as_str()).copied().unwrap_or(0),
                    moving: entry,
                    anticipate: takeoff.anticipate,
                    carry: takeoff.carry,
                });
                (PoseOp::Entry { entry: program.entries.len() as u16 - 1, into }, cost)
            }
            Motion::Landing(landing) => {
                // It reads the base's landing, which only the first layer's machines keep.
                if into != 0 {
                    issues.push(&path, "a landing plays only in the first layer's machines");
                }
                let entry = entry(&landing.standing, &landing.moving, 3, &path, names, issues);
                // The pose, and the catch's contact pose again where the weights have moved since.
                let cost = 2 * (1 + entry.len() as u16);
                program.entries.push(ProgramEntry {
                    landing: true,
                    standing: names.clips.get(landing.standing.as_str()).copied().unwrap_or(0),
                    moving: entry,
                    anticipate: false,
                    carry: false,
                });
                (PoseOp::Entry { entry: program.entries.len() as u16 - 1, into }, cost)
            }
            Motion::Machine(name) => match names.machines.get(name.as_str()) {
                Some(&inner) => (PoseOp::Machine { machine: inner, into }, 0),
                None => {
                    issues.push(&path, format!("plays machine `{name}`, which `machines` doesn't name"));
                    (PoseOp::Machine { machine: 0, into }, 0)
                }
            },
        };
        program.states[index].ops = Span { start: program.ops.len() as u16, len: 1 };
        program.ops.push(op);
        state_cost[index] = cost;
    }
    states
}

fn blend_1d(
    blend: &Blend1D,
    path: &str,
    into: u8,
    names: &Names<'_>,
    program: &mut GraphProgram,
    issues: &mut Issues,
) -> (PoseOp, u16) {
    let axis = names.number(issues, format!("{path}.parameter"), &blend.parameter);
    let samples = format!("{path}.samples");
    if blend.samples.is_empty() {
        issues.push(&samples, "a blend of no samples");
    }
    issues.limit(&samples, blend.samples.len(), MAX_BLEND_1D_SAMPLES);
    let start = program.samples.len() as u16;
    for (index, sample) in blend.samples.iter().enumerate() {
        let at = indexed(&samples, index);
        let clip = names.clip(issues, format!("{at}.clip"), &sample.clip);
        if !sample.at.is_finite() {
            issues.push(format!("{at}.at"), format!("{} isn't a place", sample.at));
        } else if index > 0 && sample.at <= blend.samples[index - 1].at {
            issues.push(format!("{at}.at"), "samples go in increasing order, each past the one before");
        }
        program.samples.push(ProgramSample { clip, at: [sample.at, 0.0], phase: 0.0, sync: true });
    }
    let sync = names.sync(issues, format!("{path}.sync"), blend.sync.as_ref());
    program.blends.push(ProgramBlend {
        axes: BlendAxes::One(axis),
        samples: Span { start, len: blend.samples.len() as u16 },
        rate: super::default_rate(),
        sync,
        footwork: None,
    });
    // Two neighbours weigh in at a time.
    (PoseOp::Blend { blend: program.blends.len() as u16 - 1, into }, blend.samples.len().min(2) as u16)
}

fn blend_2d(
    blend: &Blend2D,
    path: &str,
    into: u8,
    names: &Names<'_>,
    program: &mut GraphProgram,
    issues: &mut Issues,
) -> (PoseOp, u16) {
    let x = names.number(issues, format!("{path}.x"), &blend.x);
    let y = names.number(issues, format!("{path}.y"), &blend.y);
    if blend.x == blend.y {
        issues.push(format!("{path}.y"), "a 2D blend's axes are two parameters");
    }
    let samples = format!("{path}.samples");
    if blend.samples.is_empty() {
        issues.push(&samples, "a blend of no samples");
    }
    issues.limit(&samples, blend.samples.len(), MAX_BLEND_SAMPLES);
    let [low, high] = blend.rate;
    if !(low.is_finite() && high.is_finite() && 0.0 < low && low <= high) {
        issues.push(format!("{path}.rate"), format!("({low}, {high}) isn't a range, least to most"));
    }
    let start = program.samples.len() as u16;
    for (index, sample) in blend.samples.iter().enumerate() {
        let at = indexed(&samples, index);
        let clip = names.clip(issues, format!("{at}.clip"), &sample.clip);
        if !sample.at.iter().all(|value| value.is_finite()) {
            issues.push(format!("{at}.at"), format!("{:?} isn't a place", sample.at));
        } else if blend.samples[..index].iter().any(|other| other.at == sample.at) {
            issues.push(
                format!("{at}.at"),
                format!("another sample sits at {:?}, so neither can be exact", sample.at),
            );
        }
        if !(0.0..=1.0).contains(&sample.phase) {
            issues.push(format!("{at}.phase"), format!("{} isn't 0 to 1", sample.phase));
        }
        program.samples.push(ProgramSample { clip, at: sample.at, phase: sample.phase, sync: sample.sync });
    }
    let sync = names.sync(issues, format!("{path}.sync"), blend.sync.as_ref());
    let footwork = blend.footwork.as_ref().map(|footwork| {
        let at = format!("{path}.footwork");
        let brake =
            [0, 1].map(|side| names.clip(issues, format!("{at}.brake[{side}]"), &footwork.brake[side]));
        let launch =
            [0, 1].map(|side| names.clip(issues, format!("{at}.launch[{side}]"), &footwork.launch[side]));
        if footwork.gaits.is_empty() {
            issues.push(format!("{at}.gaits"), "footwork hands to no gait");
        }
        let gaits = footwork
            .gaits
            .iter()
            .enumerate()
            .map(|(index, gait)| {
                let gait_at = indexed(&format!("{at}.gaits"), index);
                if !(gait.contact > 0.0 && gait.contact <= 1.0) {
                    issues.push(
                        format!("{gait_at}.contact"),
                        format!("{} isn't a share of a cycle", gait.contact),
                    );
                }
                (names.clip(issues, format!("{gait_at}.clip"), &gait.clip), gait.contact)
            })
            .collect();
        let own = ProgramFootwork { brake, launch, gaits };
        // The runtime keeps one footwork for the graph, which each blend that has it turns on.
        if program.footwork.first().is_some_and(|first| *first != own) {
            issues.push(&at, "differs from an earlier blend's footwork: a graph's blends share one");
        }
        program.footwork.push(own);
        program.footwork.len() as u16 - 1
    });
    program.blends.push(ProgramBlend {
        axes: BlendAxes::Two(x, y),
        samples: Span { start, len: blend.samples.len() as u16 },
        rate: blend.rate,
        sync,
        footwork,
    });
    // Gradient bands can weigh every sample in at once, and a phrase of footwork samples one more.
    let cost = blend.samples.len() as u16 + u16::from(footwork.is_some());
    (PoseOp::Blend { blend: program.blends.len() as u16 - 1, into }, cost)
}

/// An entry blend's moving samples, checked as v1 checks them: one to `most`, a third with a short lead.
fn entry(
    standing: &str,
    moving: &[EntrySample],
    most: usize,
    path: &str,
    names: &Names<'_>,
    issues: &mut Issues,
) -> Vec<ProgramEntrySample> {
    names.clip(issues, format!("{path}.standing"), standing);
    let list = format!("{path}.moving");
    if !(1..=most).contains(&moving.len()) {
        issues.push(&list, format!("{} moving clips, where it takes one to {most}", moving.len()));
    }
    moving
        .iter()
        .enumerate()
        .map(|(index, sample)| {
            let at = indexed(&list, index);
            let clip = names.clip(issues, format!("{at}.clip"), &sample.clip);
            issues.speeds(format!("{at}.fade"), sample.fade);
            match (index == 2, sample.short_lead) {
                (true, None) => {
                    issues.push(format!("{at}.short_lead"), "a third landing sample needs a short lead")
                }
                (false, Some(_)) => {
                    issues.push(format!("{at}.short_lead"), "only a third landing sample takes one")
                }
                (true, Some(lead)) => issues.speeds(format!("{at}.short_lead"), lead),
                (false, None) => {}
            }
            ProgramEntrySample { clip, fade: sample.fade, short_lead: sample.short_lead }
        })
        .collect()
}

/// A machine's transitions, and the most condition steps and the deepest stack trying them takes.
fn compile_transitions(
    machine: &Machine,
    at: &str,
    first: u16,
    states: &BTreeMap<&str, u16>,
    names: &Names<'_>,
    program: &mut GraphProgram,
    issues: &mut Issues,
) -> (u16, u8) {
    let list = format!("{at}.transitions");
    issues.limit(&list, machine.transitions.len(), MAX_TRANSITIONS);
    let mut steps = vec![0u16; machine.states.len()];
    let mut deepest = 0u8;
    for (index, transition) in machine.transitions.iter().enumerate() {
        let path = indexed(&list, index);
        let from = if transition.from == ANY_STATE {
            None
        } else {
            match states.get(transition.from.as_str()) {
                Some(&state) => Some(first + state),
                None => {
                    issues.push(format!("{path}.from"), format!("`{}`: no such state", transition.from));
                    None
                }
            }
        };
        let to = states.get(transition.to.as_str()).map(|&state| first + state).unwrap_or_else(|| {
            issues.push(format!("{path}.to"), format!("`{}`: no such state", transition.to));
            first
        });
        issues.decay(format!("{path}.decay"), transition.decay);
        let compiled = match condition::compile(&transition.when, &names.parameters) {
            Ok(compiled) => compiled,
            Err(message) => {
                issues.push(format!("{path}.when"), message);
                continue;
            }
        };
        if compiled.finished
            && let Some(from) = from
            && !program.states[usize::from(from)].finishes
        {
            let message = format!(
                "asks whether `{}` has finished, which never does: it loops or blends",
                transition.from
            );
            issues.push(format!("{path}.when"), message);
        }
        let start = program.conditions.len() as u16;
        let len = compiled.ops.len() as u16;
        program.conditions.extend(compiled.ops);
        deepest = deepest.max(compiled.depth as u8);
        for (state, count) in steps.iter_mut().enumerate() {
            if from.is_none_or(|from| usize::from(from) == usize::from(first) + state) {
                *count += len;
            }
        }
        program.transitions.push(ProgramTransition {
            from,
            to,
            decay: transition.decay.min(u32::from(u16::MAX)) as u16,
            condition: Span { start, len },
            reads: compiled.reads,
            finished: compiled.finished,
            phase: compiled.phase,
            rules: transition.rules,
        });
    }
    (steps.into_iter().max().unwrap_or(0), deepest)
}

/// The layers' ops, and the most clips one evaluation of them samples.
fn compile_layers(
    graph: &AnimGraph,
    target: Target,
    names: &Names<'_>,
    placements: &BTreeMap<usize, Placement>,
    program: &mut GraphProgram,
    state_cost: &[u16],
    issues: &mut Issues,
) -> u16 {
    if graph.layers.is_empty() {
        issues.push("layers", "no layers: the first plays a machine over the whole body");
    }
    issues.limit("layers", graph.layers.len(), MAX_LAYERS);
    issues.names("layers", graph.layers.iter().map(|layer| layer.name.as_str()));
    let mut cost = 0u16;
    let (mut combats, mut reactions, mut additive) = (0, 0, 0);
    for (index, layer) in graph.layers.iter().enumerate() {
        let at = named("layers", &layer.name);
        let path = format!("{at}.plays");
        let base = index == 0;
        if base
            && !matches!(&layer.plays, LayerMotion::Machine(played)
                if played.mask.is_none() && played.blend == LayerBlend::Override && played.weight == 1.0)
        {
            issues.push(
                &path,
                "the first layer plays a machine over the whole body: no mask, override, weight 1",
            );
        }
        match &layer.plays {
            LayerMotion::Machine(played) => {
                let Some(&machine) = names.machines.get(played.machine.as_str()) else { continue };
                let into = placements.get(&usize::from(machine)).map_or(0, |placement| placement.register);
                program.ops.push(PoseOp::Machine { machine, into });
                let mask = names.mask(issues, format!("{path}.mask"), played.mask.as_ref());
                issues.weight(format!("{path}.weight"), played.weight);
                if !base {
                    program.ops.push(PoseOp::Layer {
                        from: into,
                        onto: 0,
                        mask,
                        blend: played.blend,
                        weight: played.weight,
                    });
                }
                let sampled = machine_cost(program, state_cost, machine);
                if played.blend == LayerBlend::Additive {
                    additive += 1;
                    // Its reference is the same machine with each clip at its own first tick, blended as
                    // the pose is: each clip sampled twice.
                    cost += sampled;
                }
                cost += sampled;
            }
            LayerMotion::Combat(combat) => {
                combats += 1;
                issues.decay(format!("{path}.decay"), combat.decay);
                issues.decay(format!("{path}.exit"), combat.exit);
                let list = format!("{path}.moves");
                issues.limit(&list, combat.moves.len(), MAX_GRAPH_MOVES);
                issues.names(&list, combat.moves.iter().map(|row| row.name.as_str()));
                for row in &combat.moves {
                    let row_at = named(&list, &row.name);
                    let clip = names.clip(issues, format!("{row_at}.clip"), &row.clip);
                    let mask = names.mask(issues, format!("{row_at}.mask"), row.mask.as_ref());
                    let decay = row.decay.unwrap_or(combat.decay);
                    if let Some(own) = row.decay {
                        issues.decay(format!("{row_at}.decay"), own);
                    }
                    let mut variants = [None; 4];
                    for (index, variant) in row.variants.iter().enumerate() {
                        let variant_at = indexed(&format!("{row_at}.variants"), index);
                        if target == Target::Simulation {
                            issues.push(
                                &variant_at,
                                "a side's clip is presentation's choice, from a hit's direction, and the \
                                 move's tables are baked from its row's clip",
                            );
                        }
                        let side = &mut variants[variant.from as usize];
                        if side.is_some() {
                            issues.push(
                                format!("{variant_at}.from"),
                                format!("a second clip from {:?}", variant.from),
                            );
                        }
                        *side = Some(names.clip(issues, format!("{variant_at}.clip"), &variant.clip));
                    }
                    program.moves.push(ProgramMove {
                        name: row.name.clone(),
                        clip,
                        mask,
                        decay: decay.min(u32::from(u16::MAX)) as u16,
                        variants,
                    });
                }
                program.moves.sort_by(|a, b| a.name.cmp(&b.name));
                program.combat = Some(ProgramCombat {
                    decay: combat.decay.min(u32::from(u16::MAX)) as u16,
                    exit: combat.exit.min(u32::from(u16::MAX)) as u16,
                });
                program.ops.push(PoseOp::Combat { onto: 0 });
                // A whole-body move replaces what's below, unsampled; one by a mask lays one clip on it.
                let masked = combat.moves.iter().any(|row| row.mask.is_some());
                let whole = combat.moves.iter().any(|row| row.mask.is_none());
                cost = (cost + u16::from(masked)).max(u16::from(whole));
            }
            LayerMotion::Additive(added) => {
                additive += 1;
                let clip = names.clip(issues, format!("{path}.clip"), &added.clip);
                let mask = names.mask(issues, format!("{path}.mask"), added.mask.as_ref());
                issues.weight(format!("{path}.weight"), added.weight);
                program.additive.push(ProgramAdditive {
                    name: layer.name.clone(),
                    clip,
                    mask,
                    weight: added.weight,
                });
                program.ops.push(PoseOp::Additive { additive: program.additive.len() as u16 - 1, onto: 0 });
                cost += 1;
            }
            LayerMotion::Reactions(table) => {
                reactions += 1;
                issues.limit(&path, table.len(), MAX_REACTIONS);
                issues.names(&path, table.iter().map(|reaction| reaction.name.as_str()));
                for reaction in table {
                    let reaction_at = named(&path, &reaction.name);
                    let clip = names.clip(issues, format!("{reaction_at}.clip"), &reaction.clip);
                    let mask = names.mask(issues, format!("{reaction_at}.mask"), reaction.mask.as_ref());
                    issues.weight(format!("{reaction_at}.weight"), reaction.weight);
                    issues.decay(format!("{reaction_at}.fade_out"), reaction.fade_out);
                    issues.decay(format!("{reaction_at}.restart"), reaction.restart);
                    program.reactions.push(ProgramReaction {
                        name: reaction.name.clone(),
                        clip,
                        mask,
                        weight: reaction.weight,
                        fade_out: reaction.fade_out.min(u32::from(u16::MAX)) as u16,
                        restart: reaction.restart.min(u32::from(u16::MAX)) as u16,
                    });
                }
                program.ops.push(PoseOp::Reactions { onto: 0 });
                // One plays at a time.
                cost += u16::from(!table.is_empty());
            }
        }
    }
    if combats > 1 {
        issues.push("layers", format!("{combats} combat layers: the simulation's moves play in one"));
    }
    if reactions > 1 {
        issues.push("layers", format!("{reactions} reactions layers: reactions play one at a time, in one"));
    }
    if additive > MAX_ADDITIVE_LAYERS {
        issues.push("layers", format!("{additive} additive layers, past the limit of {MAX_ADDITIVE_LAYERS}"));
    }
    cost
}

fn compile_events(graph: &AnimGraph, target: Target, program: &mut GraphProgram, issues: &mut Issues) {
    issues.limit("events", graph.events.len(), MAX_EVENTS);
    issues.names("events", graph.events.iter().map(|event| event.name.as_str()));
    if target == Target::Simulation {
        for event in &graph.events {
            issues.push(
                named("events", &event.name),
                "an event goes to presentation's notifies, never the simulation",
            );
        }
    }
    let printable = |name: &str| {
        !name.is_empty() && name.len() <= MAX_EVENT_NAME && name.bytes().all(|byte| byte.is_ascii_graphic())
    };
    for event in &graph.events {
        let at = named("events", &event.name);
        if !event.name.is_empty() && !printable(&event.name) {
            issues.push(
                &at,
                format!("an event's name is 1 to {MAX_EVENT_NAME} bytes of printable ASCII, no spaces"),
            );
        }
        issues.weight(format!("{at}.min_weight"), event.min_weight);
        let from = match &event.from {
            EventSource::Foot(foot) => ProgramEventSource::Foot { foot: foot.foot, contact: foot.contact },
            EventSource::Clip(name) => {
                if !printable(name) {
                    issues.push(format!("{at}.from"), format!("`{name}` can't be a clip's event"));
                }
                ProgramEventSource::Clip(name.clone())
            }
        };
        program.events.push(ProgramEvent { name: event.name.clone(), from, min_weight: event.min_weight });
    }
}

fn check_blink(blink: &Blink, issues: &mut Issues) {
    if blink.expression.is_empty() {
        issues.push("blink.expression", "no expression");
    }
    let [least, most] = blink.every;
    if least == 0 || least > most {
        issues.push("blink.every", format!("({least}, {most}) isn't a range, least to most"));
    }
    let length = u64::from(blink.close) + u64::from(blink.hold) + u64::from(blink.open);
    if length > u64::from(least) {
        issues.push("blink", format!("a blink lasts {length} ticks, longer than the {least} between"));
    }
    if blink.close == 0 && blink.open == 0 {
        issues.push("blink", "it neither closes nor opens over any time");
    }
}
