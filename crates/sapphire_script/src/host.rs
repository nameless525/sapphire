//! A simulation's script host: the VM and which hooks are bound, and the system that fires them inside a
//! fixed step.
//!
//! **A fight's scripts share its VM** (M3 C2): one for the fight, as a scenario of code binds, and one for
//! each kind of enemy whose document names one, each hearing only its own fighters' events
//! ([`ScriptScope`]). Each chunk keeps what it made to itself, since a chunk's tables are frozen and the
//! globals are, so sharing a VM shares only its memory limit, its collector and a step's interrupts, all
//! of which are the fight's. The board is the fight's too, as its flags are: every script in it reads and
//! writes the same slots.
//!
//! What a hook reads and writes, and how that reaches the checksum, is gameplay's
//! ([`sapphire_gameplay::script`]), which knows nothing of this crate. [`attach`] gives a simulation its
//! script state and adds the system that fires hooks to [`CombatSet::Hooks`], and from then on a step goes: gameplay
//! gathers events, this answers them, gameplay applies what was asked for. After each step, in
//! [`BetweenSteps`](sapphire_gameplay::BetweenSteps), it steps the VM's collector once, so whatever steps the simulation, a game or a replay's
//! runner, collects as a game does.

use std::time::Duration;

use sapphire_bytecode::compile::CookedScript;
use sapphire_core::Name;
use sapphire_core::alloc;
use sapphire_dialogue::{ConditionId, Dialogue};
use sapphire_ecs::prelude::*;
use sapphire_gameplay::script::HookEnding;
use sapphire_gameplay::{
    CombatSet, HookEvent, HookFrame, HookWants, LendsFrame, ScriptScope, ScriptWorld, SimRng, Simulation,
    StateHasher,
};
use sapphire_math::Pcg32;
use sapphire_time::Stopwatch;

use crate::bindings::{Binding, HOOK_COUNT, HOOKS, ON_BEAT, ON_DEFEAT, ON_HIT, ON_MOVE, ON_PHASE};
use crate::vm::{Hook, HookEnd, Limits, ScriptVm, VmError};

/// A script host's counters (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptStats {
    pub hooks_fired: u64,
    pub hooks_failed: u64,
    pub hooks_over_budget: u64,
    /// Hooks that didn't run because their step's interrupts had gone.
    pub hooks_skipped: u64,
    /// Commands taken back because the hook that asked ended badly.
    pub commands_discarded: u64,
    /// The VM's allocations inside hooks: their own budget, not gate 4's.
    pub script_allocations: u64,
    pub script_bytes: u64,
    /// The most any one hook allocated in the VM.
    pub worst_hook_allocations: u64,
    /// How long the latest step with hooks spent on them, by wall clock. Nothing reads this back.
    pub last_step: Duration,
    pub longest_step: Duration,
    /// Steps that spent more than [`Limits::step_budget`] (spec §3.2).
    pub over_budget_steps: u64,
    /// Collection steps taken between steps, and how long the latest and the longest took.
    pub collections: u64,
    pub last_collection: Duration,
    pub longest_collection: Duration,
    /// All the time spent on hooks and on collection so far, by wall clock: what a bench takes the difference
    /// of across a frame, for §3.2's script row (M3 Z1).
    pub hooks_time: Duration,
    pub collections_time: Duration,
    /// A dialogue's conditions asked between steps ([`ask`]), and those that didn't end well: a fault,
    /// an error, over budget, or asked by a number its set doesn't have. Those read false.
    pub conditions_asked: u64,
    pub conditions_faulted: u64,
}

/// A dialogue's conditions, bound in a host's VM by [`ScriptHost::bind_conditions`]: functions of no
/// arguments that read flags by their place in `names`, as hooks read moves. Asked through [`ask`]. The
/// default is a dialogue's with none, which binds nothing.
#[derive(Default)]
pub struct ConditionSet {
    functions: Vec<Hook>,
    /// The chunk's `names`, which [`ask`] puts on the frame for the time of one question.
    names: Vec<Name>,
}

impl ConditionSet {
    /// How many there are.
    #[must_use]
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether there are none, as for a dialogue with no condition, which binds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }
}

/// A chunk bound in a host's VM: its hooks, and whose events they hear.
struct Bound {
    scope: ScriptScope,
    /// Its hooks, by [`HOOKS`]' position; swapped whole when it's bound again, between steps.
    hooks: [Option<Hook>; HOOK_COUNT],
    /// The hash of its bytes.
    chunk: u64,
    /// The name it was bound by, which a reload goes by.
    name: String,
    /// Its `names`, which go on the frame for the time one of its hooks runs.
    names: Vec<Name>,
}

impl Bound {
    /// Its source file name: its chunk name without the `=`.
    fn source_name(&self) -> &str {
        self.name.strip_prefix('=').unwrap_or(&self.name)
    }
}

/// One simulation's VM and what's bound to it, as a resource in its world: a system takes it by `ResMut`,
/// so firing a hook costs no lock, and whoever swaps a chunk or steps the collector does it between steps
/// through the simulation that owns it.
#[derive(Resource)]
pub struct ScriptHost {
    vm: ScriptVm,
    /// What's bound, one chunk for each scope at most: the fight's first, if there is one, then each kind's in
    /// the order they were bound, which is the order they answer an event in.
    bound: Vec<Bound>,
    /// See [`ScriptWorld::set_beat_every`].
    beat_every: u64,
    stats: ScriptStats,
    error: Option<String>,
}

impl ScriptHost {
    /// A host with nothing bound.
    pub fn new(limits: Limits) -> Result<Self, VmError> {
        Self::new_with_bindings(limits, &[])
    }

    /// [`Self::new`], with more bindings: see [`ScriptVm::with_bindings`].
    pub fn new_with_bindings(limits: Limits, more: &[Binding]) -> Result<Self, VmError> {
        Ok(Self {
            vm: ScriptVm::with_bindings(limits, more)?,
            bound: Vec::new(),
            beat_every: 0,
            stats: ScriptStats::default(),
            error: None,
        })
    }

    /// Fires every bound `on_beat` every this many ticks, from the moment it's [`attach`]ed. A scenario's
    /// choice, so it's in the replay's `tables_hash`.
    pub const fn beat_every(&mut self, ticks: u64) {
        self.beat_every = ticks;
    }

    /// Runs a cooked chunk and binds what it returns as the fight's script, which hears every event,
    /// replacing the fight's hooks: [`Self::bind_for`] with [`ScriptScope::Fight`].
    pub fn bind(&mut self, cooked: &CookedScript, name: &str) -> Result<(), VmError> {
        self.bind_for(ScriptScope::Fight, cooked, name)
    }

    /// Runs a cooked chunk and binds what it returns for `scope`, replacing what that scope had bound and
    /// leaving every other's. Before [`attach`]: on an attached host, [`rebind`] is what tells the
    /// simulation. `name` is its chunk name, which errors show: `=` and the source's file name, so that a
    /// reload finds it ([`Self::source_names`]).
    pub fn bind_for(&mut self, scope: ScriptScope, cooked: &CookedScript, name: &str) -> Result<(), VmError> {
        let bound = self.load(scope, cooked, name)?;
        self.install(bound);
        Ok(())
    }

    /// Runs a chunk and takes its hooks and names, binding nothing yet.
    fn load(&self, scope: ScriptScope, cooked: &CookedScript, name: &str) -> Result<Bound, VmError> {
        let table = self.vm.load(cooked, name)?;
        let mut hooks = [const { None }; HOOK_COUNT];
        for (slot, (hook, _)) in hooks.iter_mut().zip(HOOKS) {
            *slot = table.hook(hook)?;
        }
        let names = table.names()?.iter().map(|name| Name::intern(name)).collect();
        Ok(Bound { scope, hooks, chunk: chunk_hash(cooked.as_ref()), name: name.to_owned(), names })
    }

    /// Binds what [`Self::load`] took, in its scope's place.
    fn install(&mut self, bound: Bound) {
        match self.bound.iter_mut().find(|held| held.scope == bound.scope) {
            Some(held) => *held = bound,
            None if bound.scope == ScriptScope::Fight => self.bound.insert(0, bound),
            None => self.bound.push(bound),
        }
    }

    /// Each bound chunk's source file name, its chunk name without the `=`, in the order they answer: the
    /// fight's, then each kind's. Two kinds that name one file show it twice.
    pub fn source_names(&self) -> impl Iterator<Item = &str> {
        self.bound.iter().map(Bound::source_name)
    }

    /// Whose events each bound chunk hears, in the order they answer.
    pub fn scopes(&self) -> impl Iterator<Item = ScriptScope> + '_ {
        self.bound.iter().map(|bound| bound.scope)
    }

    /// Compiles a dialogue's conditions from the source its cook kept, with this build's pinned compiler and
    /// its scan, runs the chunk between steps and keeps what it makes. Nothing about the hooks changes, and
    /// nothing reaches `tables_hash`: a condition decides only what a dialogue shows, never the simulation. A
    /// dialogue with no conditions binds nothing and compiles nothing.
    ///
    /// The chunk is made here, in this process, from text, so the VM never loads bytes a file supplied (M3
    /// Q2b): a cooked dialogue may be anyone's, and Luau's loader trusts its bytecode's structure. It costs a
    /// compile, once a dialogue, which `examples/dialogue_bench.rs` measures.
    ///
    /// # Errors
    ///
    /// If the source doesn't compile or fails the scan, doesn't load, isn't a table of `conditions` and
    /// `names`, or makes a different number of conditions than the graph asks.
    pub fn bind_conditions(&self, dialogue: &Dialogue, name: &str) -> Result<ConditionSet, VmError> {
        let expected = usize::from(dialogue.presentation.conditions);
        if expected == 0 {
            return Ok(ConditionSet { functions: Vec::new(), names: Vec::new() });
        }
        let chunk = {
            let _vm = alloc::VmAllocations::enter();
            sapphire_bytecode::cook(&dialogue.presentation.source)?
        };
        let table = self.vm.load(&chunk, name)?;
        let functions = table.conditions()?;
        if functions.len() != expected {
            return Err(VmError::ConditionCount { name: name.to_owned(), expected, found: functions.len() });
        }
        let names = table.names()?;
        let _vm = alloc::VmAllocations::enter();
        Ok(ConditionSet { functions, names: names.iter().map(|name| Name::intern(name)).collect() })
    }

    /// Asks one condition on a frame lent read-only, with the set's names on it for the time it runs.
    fn ask_condition(
        &mut self,
        conditions: &mut ConditionSet,
        id: ConditionId,
        frame: &mut HookFrame,
    ) -> bool {
        self.stats.conditions_asked += 1;
        let Some(function) = conditions.functions.get(usize::from(id.0)) else {
            self.stats.conditions_faulted += 1;
            return false;
        };
        frame.fault = false;
        frame.swap_names(&mut conditions.names);
        let (end, holds) = self.vm.ask(frame, function);
        frame.swap_names(&mut conditions.names);
        let faulted = frame.fault;
        if let HookEnd::Failed(error) = &end {
            self.error = Some(error.clone());
        }
        let well = end == HookEnd::Returned && !faulted;
        self.stats.conditions_faulted += u64::from(!well);
        well && holds
    }

    /// The VM.
    #[must_use]
    pub const fn vm(&self) -> &ScriptVm {
        &self.vm
    }

    /// The counters.
    #[must_use]
    pub const fn stats(&self) -> ScriptStats {
        self.stats
    }

    /// The last error a hook raised, for a log. Its text isn't in the checksum.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// One piece of collection, outside a step: see [`ScriptVm::gc_step`]. An attached host takes one after
    /// every step by itself.
    pub fn gc_step(&self) -> Result<bool, VmError> {
        let _vm = alloc::VmAllocations::enter();
        self.vm.gc_step()
    }

    /// The step of collection a tick boundary takes, counted. A collector that can't step is a VM gone
    /// wrong, which the log hears of; the fight doesn't, since nothing simulated is in the VM.
    fn collect(&mut self) {
        let stopwatch = Stopwatch::start();
        if let Err(error) = self.gc_step() {
            self.error = Some(error.to_string());
        }
        let spent = stopwatch.elapsed();
        self.stats.collections += 1;
        self.stats.collections_time += spent;
        self.stats.last_collection = spent;
        self.stats.longest_collection = self.stats.longest_collection.max(spent);
    }

    /// Answers this step's events, in order, each by every bound chunk that hears it, in theirs, until
    /// they're done or the step's interrupts have gone.
    fn run(&mut self, scripts: &mut ScriptWorld, rng: &mut Pcg32) {
        let count = scripts.events().len();
        if count == 0 {
            return;
        }
        let stopwatch = Stopwatch::start();
        let limits = self.vm.limits();
        let spent_before = ScriptVm::interrupts();
        scripts.frame_mut().rng = *rng;
        for index in 0..count {
            let Some(event) = scripts.events().nth(index) else { break };
            let hook = hook_of(event);
            for bound in 0..self.bound.len() {
                let answers = &self.bound[bound];
                if answers.hooks[hook].is_none() || !answers.scope.hears(event, scripts.frame_mut()) {
                    continue;
                }
                if ScriptVm::interrupts().wrapping_sub(spent_before) >= limits.step_interrupts {
                    self.stats.hooks_skipped += 1;
                    scripts.record_end(index, HookEnding::Skipped);
                    continue;
                }
                let frame = scripts.frame_mut();
                frame.swap_names(&mut self.bound[bound].names);
                let args = args_of(event, frame);
                let ending = self.fire(bound, hook, args, frame);
                frame.swap_names(&mut self.bound[bound].names);
                scripts.record_end(index, ending);
            }
        }
        *rng = scripts.frame_mut().rng;
        let spent = stopwatch.elapsed();
        self.stats.last_step = spent;
        self.stats.hooks_time += spent;
        self.stats.longest_step = self.stats.longest_step.max(spent);
        self.stats.over_budget_steps += u64::from(spent > limits.step_budget);
    }

    /// Runs one of a bound chunk's hooks. One that doesn't end well leaves no commands and doesn't touch
    /// the board.
    fn fire(
        &mut self,
        bound: usize,
        hook: usize,
        args: (f64, f64, f64),
        frame: &mut HookFrame,
    ) -> HookEnding {
        let Some(function) = self.bound[bound].hooks[hook].as_ref() else { return HookEnding::Returned };
        let before = alloc::vm_counts();
        frame.fault = false;
        let queued_before = frame.queued().len();
        let board_before = frame.board;
        let end = self.vm.fire(frame, function, args);
        let ending = match (&end, frame.fault) {
            (HookEnd::Returned, false) => HookEnding::Returned,
            (HookEnd::Returned, true) => HookEnding::Faulted,
            (HookEnd::OverBudget, _) => HookEnding::OverBudget,
            (HookEnd::Failed(_), _) => HookEnding::Failed,
        };
        if ending != HookEnding::Returned {
            self.stats.commands_discarded += (frame.queued().len() - queued_before) as u64;
            frame.truncate_queue(queued_before);
            frame.board = board_before;
        }
        match end {
            HookEnd::Returned => {}
            HookEnd::OverBudget => self.stats.hooks_over_budget += 1,
            HookEnd::Failed(error) => {
                self.stats.hooks_failed += 1;
                self.error = Some(error);
            }
        }
        let spent = alloc::vm_counts();
        let allocations = spent.allocations.wrapping_sub(before.allocations);
        self.stats.hooks_fired += 1;
        self.stats.script_allocations += allocations;
        self.stats.script_bytes += spent.bytes.wrapping_sub(before.bytes);
        self.stats.worst_hook_allocations = self.stats.worst_hook_allocations.max(allocations);
        ending
    }
}

/// The hash of a chunk's bytes, which `tables_hash` holds.
fn chunk_hash(cooked: &[u8]) -> u64 {
    let mut hash = StateHasher::new();
    hash.bytes(cooked);
    hash.finish()
}

/// Which hook answers an event.
const fn hook_of(event: HookEvent) -> usize {
    match event {
        HookEvent::Phase { .. } => ON_PHASE,
        HookEvent::MoveStarted { .. } => ON_MOVE,
        HookEvent::Hit { .. } => ON_HIT,
        HookEvent::Defeated { .. } => ON_DEFEAT,
        HookEvent::Beat { .. } => ON_BEAT,
    }
}

/// What an event's hook is handed, as `api.toml` declares each, with the answering chunk's names on
/// `frame`. A hook reads as many of these as it names.
fn args_of(event: HookEvent, frame: &HookFrame) -> (f64, f64, f64) {
    let id = |actor: sapphire_gameplay::ActorId| f64::from(actor.raw());
    match event {
        HookEvent::Phase { actor, phase, health } => (id(actor), f64::from(phase), f64::from(health)),
        HookEvent::MoveStarted { actor, name } => (id(actor), frame.place_of(name) as f64, 0.0),
        HookEvent::Hit { attacker, target, damage } => (id(attacker), id(target), f64::from(damage)),
        HookEvent::Defeated { actor } => (id(actor), 0.0, 0.0),
        HookEvent::Beat { tick } => (tick as f64, 0.0, 0.0),
    }
}

/// Puts `host` in a simulation, between steps, and adds the system that fires its hooks, the only one in
/// [`CombatSet::Hooks`], and the one that steps its collector after every step. Attaching again replaces
/// the host and keeps the two systems.
pub fn attach(sim: &mut Simulation, host: ScriptHost) {
    let installed = sim.world().contains_resource::<ScriptAttached>();
    sim.enable_scripts();
    sim.insert_resource(host);
    tell(sim);
    if !installed {
        sim.insert_resource(ScriptAttached);
        sim.add_combat_system(CombatSet::Hooks, fire_hooks);
        sim.add_between_steps_system(collect);
    }
}

/// Tells a simulation's script state what its host has bound: an event is gathered if any chunk's hook
/// wants it.
fn tell(sim: &mut Simulation) {
    let host = host_of(sim);
    let any = |hook: usize| host.bound.iter().any(|bound| bound.hooks[hook].is_some());
    let wants =
        HookWants { phases: any(ON_PHASE), moves: any(ON_MOVE), hits: any(ON_HIT), defeats: any(ON_DEFEAT) };
    let chunks: Vec<(ScriptScope, u64)> = host.bound.iter().map(|bound| (bound.scope, bound.chunk)).collect();
    let beat_every = if any(ON_BEAT) { host.beat_every } else { 0 };
    let mut scripts = sim.scripts_mut().expect("whoever attaches a host enables scripts");
    scripts.bind(wants, &chunks);
    scripts.set_beat_every(beat_every);
}

/// Binds a new chunk for `scope` on an attached host, between steps: the reload. The board stays, with
/// everything else outside the VM, and so does every other scope's chunk, and the simulation's
/// `tables_hash` says what's bound now, as it does for a live edit of moves.
///
/// # Errors
///
/// If the chunk doesn't load, in which case what was bound stays.
///
/// # Panics
///
/// If nothing's been attached.
pub fn rebind(
    sim: &mut Simulation,
    scope: ScriptScope,
    cooked: &CookedScript,
    name: &str,
) -> Result<(), VmError> {
    host_of_mut(sim).bind_for(scope, cooked, name)?;
    tell(sim);
    Ok(())
}

/// Installation belongs to the simulation, even when the host resource is replaced.
#[derive(Resource)]
struct ScriptAttached;

/// What a [`reload`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reloaded {
    /// The new chunk is bound, and the simulation's `tables_hash` says so.
    Replaced,
    /// It cooked to what's bound already, so nothing changed: no new `tables_hash`, no edit to end a
    /// recording at.
    Unchanged,
}

/// A script's source, cooked as the cooker does it ([`cook`](crate::cook): the pinned compiler, then the scan)
/// and bound on an attached host between steps, wherever a chunk of that file is bound, keeping each one's
/// scope: what hot reload and live link bring (M3 L3). `source_name` is the file's name, which a host's
/// chunk name must be (`=` and it), so an edit can't land on another script. The board and everything else
/// outside the VM stay, so the fight goes on from the same numbers with new decisions.
///
/// # Errors
///
/// If the simulation runs no script, none of the host's came from this file, the source doesn't cook or
/// the chunk doesn't load. What was bound stays, all of it.
pub fn reload(sim: &mut Simulation, source_name: &str, source: &str) -> Result<Reloaded, ReloadError> {
    let Some(host) = sim.world().get_resource::<ScriptHost>() else {
        return Err(ReloadError::NoScript { pushed: source_name.to_owned() });
    };
    let scopes: Vec<ScriptScope> = host
        .bound
        .iter()
        .filter(|bound| bound.source_name() == source_name)
        .map(|bound| bound.scope)
        .collect();
    if scopes.is_empty() {
        let bound = host.source_names().collect::<Vec<_>>().join(", ");
        return Err(ReloadError::NotBound { pushed: source_name.to_owned(), bound });
    }
    let cooked = sapphire_bytecode::compile::cook(source).map_err(VmError::Cook)?;
    let chunk = chunk_hash(cooked.as_ref());
    if host.bound.iter().filter(|bound| scopes.contains(&bound.scope)).all(|bound| bound.chunk == chunk) {
        return Ok(Reloaded::Unchanged);
    }
    // Every one loads before any is bound, so a chunk that can't leaves all of them as they were.
    let name = format!("={source_name}");
    let loaded =
        scopes.iter().map(|&scope| host.load(scope, &cooked, &name)).collect::<Result<Vec<_>, _>>()?;
    let mut host = host_of_mut(sim);
    for bound in loaded {
        host.install(bound);
    }
    tell(sim);
    Ok(Reloaded::Replaced)
}

/// Why a [`reload`] kept what was bound.
#[derive(Debug, thiserror::Error)]
pub enum ReloadError {
    #[error("{pushed}: this fight runs no script")]
    NoScript { pushed: String },
    /// `bound` is the file names of what's bound, joined by commas.
    #[error("{pushed} isn't a script this fight runs, which are {bound:?}")]
    NotBound { pushed: String, bound: String },
    #[error(transparent)]
    Vm(#[from] VmError),
}

fn fire_hooks(
    mut host: ResMut<'_, ScriptHost>,
    mut scripts: ResMut<'_, ScriptWorld>,
    mut rng: ResMut<'_, SimRng>,
) {
    host.run(&mut scripts, &mut rng.0);
}

fn collect(mut host: ResMut<'_, ScriptHost>) {
    host.collect();
}

/// Asks one of a dialogue's conditions, between steps, on the flags as they stand: whether a line or a
/// choice shows. A question never moves the simulation: the frame it runs on is read-only, so a command,
/// a board write or a draw is a fault, and a condition that faults, fails or runs out of budget doesn't
/// hold. False on a simulation with no host attached. `on` is a simulation, or a replay's recording or
/// player of one.
pub fn ask(on: &mut impl LendsFrame, conditions: &mut ConditionSet, id: ConditionId) -> bool {
    on.read_only_frame::<ScriptHost, _>(|host, frame| host.ask_condition(conditions, id, frame))
        .unwrap_or(false)
}

/// The host [`attach`] put in `sim`, to read: its stats, a collection step between steps.
///
/// # Panics
///
/// If nothing's been attached.
#[must_use]
pub fn host_of(sim: &Simulation) -> &ScriptHost {
    sim.world().resource::<ScriptHost>()
}

/// The host [`attach`] put in `sim`, to change between steps.
///
/// # Panics
///
/// If nothing's been attached.
pub fn host_of_mut(sim: &mut Simulation) -> Mut<'_, ScriptHost> {
    sim.resource_mut::<ScriptHost>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cook;

    /// A reload that doesn't load leaves what was bound, and the board is the simulation's, so it stays
    /// whatever the host does.
    #[test]
    fn failed_reloads_preserve_old_hooks_and_nonzero_board_state() {
        let mut host = ScriptHost::new(Limits { interrupts: 20, ..Limits::default() }).unwrap();
        host.bind(
            &cook("return {on_beat = function() sapphire.set_board(0, sapphire.board(0) + 1) end}").unwrap(),
            "=old",
        )
        .unwrap();
        let mut frame = HookFrame::new(0);
        host.fire(0, ON_BEAT, (0.0, 0.0, 0.0), &mut frame);
        assert_eq!(frame.board[0], 1.0);
        for source in ["return {on_phase = function() end, on_beat = 42}", "while true do end return {}"] {
            assert!(host.bind(&cook(source).unwrap(), "=bad").is_err());
            host.fire(0, ON_BEAT, (0.0, 0.0, 0.0), &mut frame);
        }
        assert_eq!(frame.board[0], 3.0);
        assert_eq!(host.stats.hooks_fired, 3);
    }

    #[test]
    fn attaching_twice_replaces_the_host_without_firing_twice_per_step() {
        use sapphire_gameplay::{SimConfig, TickActions};

        let mut sim = Simulation::new(SimConfig::default());
        for _ in 0..2 {
            let mut host = ScriptHost::new(Limits::default()).unwrap();
            host.bind(
                &cook("return {on_beat = function() sapphire.set_board(0, sapphire.board(0) + 1) end}")
                    .unwrap(),
                "=beat",
            )
            .unwrap();
            host.beat_every(1);
            attach(&mut sim, host);
        }
        sim.step(&TickActions::default());
        assert_eq!(sim.scripts().unwrap().board()[0], 1.0);
        assert_eq!(host_of(&sim).stats.hooks_fired, 1);
    }
}
