//! A simulation's script host: the VM and which hooks are bound, and the system that fires them inside a
//! fixed step.
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
use sapphire_ecs::prelude::*;
use sapphire_gameplay::script::HookEnding;
use sapphire_gameplay::{
    CombatSet, HookEvent, HookFrame, HookWants, ScriptWorld, SimRng, Simulation, StateHasher,
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
}

/// One simulation's VM and what's bound to it, as a resource in its world: a system takes it by `ResMut`,
/// so firing a hook costs no lock, and whoever swaps a chunk or steps the collector does it between steps
/// through the simulation that owns it.
#[derive(Resource)]
pub struct ScriptHost {
    vm: ScriptVm,
    /// The bound hooks, by [`HOOKS`]' position; swapped whole by [`Self::bind`], between steps.
    hooks: [Option<Hook>; HOOK_COUNT],
    /// The hash of the bound chunk's bytes.
    chunk: u64,
    /// The name it was bound by, which a reload goes by.
    name: String,
    /// The bound chunk's `names`.
    names: Vec<Name>,
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
            hooks: [const { None }; HOOK_COUNT],
            chunk: 0,
            name: String::new(),
            names: Vec::new(),
            beat_every: 0,
            stats: ScriptStats::default(),
            error: None,
        })
    }

    /// Fires `on_beat` every this many ticks, from the moment it's [`attach`]ed. A scenario's choice,
    /// so it's in the replay's `tables_hash`.
    pub const fn beat_every(&mut self, ticks: u64) {
        self.beat_every = ticks;
    }

    /// Runs a cooked chunk and binds what it returns, replacing every hook. Before [`attach`]: on an attached
    /// host, [`rebind`] is what tells the simulation. `name` is its chunk name, which errors show: `=` and the
    /// source's file name, so that a reload finds it ([`Self::source_name`]).
    pub fn bind(&mut self, cooked: &CookedScript, name: &str) -> Result<(), VmError> {
        let table = self.vm.load(cooked, name)?;
        let mut hooks = [const { None }; HOOK_COUNT];
        for (slot, (hook, _)) in hooks.iter_mut().zip(HOOKS) {
            *slot = table.hook(hook)?;
        }
        self.names = table.names()?.iter().map(|name| Name::intern(name)).collect();
        self.hooks = hooks;
        let mut hash = StateHasher::new();
        hash.bytes(cooked.as_ref());
        self.chunk = hash.finish();
        name.clone_into(&mut self.name);
        Ok(())
    }

    /// The bound chunk's source file name: its chunk name without the `=`. Empty if nothing's bound.
    #[must_use]
    pub fn source_name(&self) -> &str {
        self.name.strip_prefix('=').unwrap_or(&self.name)
    }

    /// Whether `cooked` is what's bound, byte for byte, as far as a hash can say.
    fn holds(&self, cooked: &[u8]) -> bool {
        let mut hash = StateHasher::new();
        hash.bytes(cooked);
        !self.name.is_empty() && hash.finish() == self.chunk
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
        self.stats.last_collection = spent;
        self.stats.longest_collection = self.stats.longest_collection.max(spent);
    }

    /// Answers this step's events, in order, until they're done or the step's interrupts have gone.
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
            if ScriptVm::interrupts().wrapping_sub(spent_before) >= limits.step_interrupts {
                self.stats.hooks_skipped += 1;
                scripts.record_end(index, HookEnding::Skipped);
                continue;
            }
            let (hook, args) = answer(event, scripts.frame_mut());
            if self.hooks[hook].is_some() {
                let ending = self.fire(hook, args, scripts.frame_mut());
                scripts.record_end(index, ending);
            }
        }
        *rng = scripts.frame_mut().rng;
        let spent = stopwatch.elapsed();
        self.stats.last_step = spent;
        self.stats.longest_step = self.stats.longest_step.max(spent);
        self.stats.over_budget_steps += u64::from(spent > limits.step_budget);
    }

    /// Runs one hook. One that doesn't end well leaves no commands and doesn't touch the board.
    fn fire(&mut self, hook: usize, args: (f64, f64, f64), frame: &mut HookFrame) -> HookEnding {
        let Some(function) = self.hooks[hook].as_ref() else { return HookEnding::Returned };
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

/// Which hook answers an event, and what it's handed, as `api.toml` declares each. A hook reads as many of
/// these as it names.
fn answer(event: HookEvent, frame: &HookFrame) -> (usize, (f64, f64, f64)) {
    let id = |actor: sapphire_gameplay::ActorId| f64::from(actor.raw());
    match event {
        HookEvent::Phase { actor, phase, health } => {
            (ON_PHASE, (id(actor), f64::from(phase), f64::from(health)))
        }
        HookEvent::MoveStarted { actor, name } => (ON_MOVE, (id(actor), frame.place_of(name) as f64, 0.0)),
        HookEvent::Hit { attacker, target, damage } => {
            (ON_HIT, (id(attacker), id(target), f64::from(damage)))
        }
        HookEvent::Defeated { actor } => (ON_DEFEAT, (id(actor), 0.0, 0.0)),
        HookEvent::Beat { tick } => (ON_BEAT, (tick as f64, 0.0, 0.0)),
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

/// Tells a simulation's script state what its host has bound.
fn tell(sim: &mut Simulation) {
    let host = host_of(sim);
    let wants = HookWants {
        phases: host.hooks[ON_PHASE].is_some(),
        moves: host.hooks[ON_MOVE].is_some(),
        hits: host.hooks[ON_HIT].is_some(),
        defeats: host.hooks[ON_DEFEAT].is_some(),
    };
    let (chunk, names) = (host.chunk, host.names.clone());
    let beat_every = if host.hooks[ON_BEAT].is_some() { host.beat_every } else { 0 };
    let mut scripts = sim.scripts_mut().expect("whoever attaches a host enables scripts");
    scripts.bind(wants, chunk, names);
    scripts.set_beat_every(beat_every);
}

/// Binds a new chunk on an attached host, between steps: the reload. The board stays, with everything else
/// outside the VM, and the simulation's `tables_hash` says what's bound now, as it does for a live edit of
/// moves.
///
/// # Errors
///
/// If the chunk doesn't load, in which case what was bound stays.
///
/// # Panics
///
/// If nothing's been attached.
pub fn rebind(sim: &mut Simulation, cooked: &CookedScript, name: &str) -> Result<(), VmError> {
    host_of_mut(sim).bind(cooked, name)?;
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
/// and bound on an attached host between steps: what hot reload and live link bring (M3 L3). `source_name`
/// is the file's name, which a host's chunk name must be (`=` and it), so an edit can't land on another
/// script. The board and everything else outside the VM stay, so the fight goes on from the same numbers
/// with new decisions.
///
/// # Errors
///
/// If the simulation runs no script, the host's came from another file, the source doesn't cook or the
/// chunk doesn't load. What was bound stays.
pub fn reload(sim: &mut Simulation, source_name: &str, source: &str) -> Result<Reloaded, ReloadError> {
    let Some(host) = sim.world().get_resource::<ScriptHost>() else {
        return Err(ReloadError::NoScript { pushed: source_name.to_owned() });
    };
    let bound = host.source_name();
    if bound != source_name {
        return Err(ReloadError::NotBound { pushed: source_name.to_owned(), bound: bound.to_owned() });
    }
    let cooked = sapphire_bytecode::compile::cook(source).map_err(VmError::Cook)?;
    if host_of(sim).holds(cooked.as_ref()) {
        return Ok(Reloaded::Unchanged);
    }
    rebind(sim, &cooked, &format!("={source_name}"))?;
    Ok(Reloaded::Replaced)
}

/// Why a [`reload`] kept what was bound.
#[derive(Debug, thiserror::Error)]
pub enum ReloadError {
    #[error("{pushed}: this fight runs no script")]
    NoScript { pushed: String },
    #[error("{pushed} isn't the script this fight runs, which is {bound:?}")]
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
        host.fire(ON_BEAT, (0.0, 0.0, 0.0), &mut frame);
        assert_eq!(frame.board[0], 1.0);
        for source in ["return {on_phase = function() end, on_beat = 42}", "while true do end return {}"] {
            assert!(host.bind(&cook(source).unwrap(), "=bad").is_err());
            host.fire(ON_BEAT, (0.0, 0.0, 0.0), &mut frame);
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
