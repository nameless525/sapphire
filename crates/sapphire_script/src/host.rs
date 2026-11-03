//! A simulation's script host: which hooks are bound, what they keep and what they asked for, and the
//! system that fires them inside a fixed step.
//!
//! Gameplay knows nothing of this crate: [`attach`] puts the host in the world as a resource and adds its
//! system with [`Simulation::add_combat_system`]. It reads and writes through gameplay's public
//! resources, and adds what it keeps to the checksum beside [`Simulation::checksum`]. At L2 the plain
//! parts ([`Command`], the board, the events) move to gameplay so that the checksum can hold them
//! itself; nothing here needs Luau to name them.

use mlua::{Function, Table};
use sapphire_core::alloc;
use sapphire_ecs::prelude::*;
use sapphire_gameplay::{ActorTable, CombatSet, Fighter, SimRng, Simulation, SlowMotion, StateHasher};

use crate::bindings::{BOARD_SLOTS, Binding, Command, FRAME_ACTORS, Frame};
use crate::vm::{HookEnd, Limits, ScriptVm, VmError};

/// Hooks a chunk may hold, by the name it returns them under.
pub const HOOKS: [&str; 2] = ["on_phase", "on_beat"];

const ON_PHASE: usize = 0;
const ON_BEAT: usize = 1;

/// Health a phase lasts, as a fraction: `on_phase` fires as an actor's health crosses each quarter.
const PHASES: u32 = 4;

/// A script host's counters (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScriptStats {
    pub hooks_fired: u64,
    pub hooks_failed: u64,
    pub hooks_over_budget: u64,
    pub commands_applied: u64,
    pub commands_discarded: u64,
    /// Commands past the end of a hook's queue: the same on every run, so not a checksum's business.
    pub commands_dropped: u64,
    /// The VM's allocations inside hooks: its own budget, not gate 4's.
    pub script_allocations: u64,
    pub script_bytes: u64,
    /// The most any one hook allocated in the VM.
    pub worst_hook_allocations: u64,
}

/// One simulation's VM and what its hooks keep, as a resource in its world: a system takes it by
/// `ResMut`, so firing a hook costs no lock, and whoever swaps a chunk or steps the collector does it
/// between steps through the simulation that owns it.
#[derive(Resource)]
pub struct ScriptHost {
    vm: ScriptVm,
    /// The bound hooks, by [`HOOKS`]' position; swapped whole by [`Self::bind`], between steps.
    hooks: [Option<Function>; HOOKS.len()],
    frame: Frame,
    /// The phase each actor last fired for, by the order the system sees them.
    phases: [u8; FRAME_ACTORS],
    /// `on_beat` fires every this many ticks, or never at 0.
    beat_every: u64,
    tick: u64,
    /// Every applied command so far, chained.
    commands_hash: u64,
    /// Every hook's end so far, chained, so a hook that fails on one run fails the checksum.
    ends_hash: u64,
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
            hooks: [const { None }; HOOKS.len()],
            frame: Frame::new(),
            phases: [0; FRAME_ACTORS],
            beat_every: 0,
            tick: 0,
            commands_hash: 0,
            ends_hash: 0,
            stats: ScriptStats::default(),
            error: None,
        })
    }

    /// Fires `on_beat` every this many ticks.
    pub const fn beat_every(&mut self, ticks: u64) {
        self.beat_every = ticks;
    }

    /// Runs a cooked chunk and binds what it returns, replacing every hook, on the next step's terms:
    /// the reload. The board stays, and so does anything else outside the VM.
    pub fn bind(&mut self, cooked: &[u8], name: &str) -> Result<(), VmError> {
        let table: Table = self.vm.load(cooked, name)?;
        let mut hooks = [const { None }; HOOKS.len()];
        for (slot, hook) in hooks.iter_mut().zip(HOOKS) {
            *slot = table.get::<Option<Function>>(hook)?;
        }
        self.hooks = hooks;
        Ok(())
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

    /// The numbers that last.
    #[must_use]
    pub const fn board(&self) -> &[f64; BOARD_SLOTS] {
        &self.frame.board
    }

    /// The last error a hook raised, for a log. Its text isn't in the checksum.
    #[must_use]
    pub fn last_error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// What scripts add to the checksum: the board, by bits, and the chain of commands and ends.
    #[must_use]
    pub fn digest(&self) -> u64 {
        let mut hash = StateHasher::new();
        for value in &self.frame.board {
            write_f64(&mut hash, *value);
        }
        hash.u64(self.commands_hash);
        hash.u64(self.ends_hash);
        hash.finish()
    }

    /// One piece of collection, outside a step: see [`ScriptVm::gc_step`].
    pub fn gc_step(&self) -> Result<bool, VmError> {
        let _vm = alloc::VmAllocations::enter();
        self.vm.gc_step()
    }

    /// Runs what's due at this step, with the frame's numbers set. The commands stay in the frame for
    /// [`Self::apply_commands`].
    fn run(&mut self, health: &[(u32, u32); FRAME_ACTORS], rng: &mut sapphire_math::Pcg32) {
        self.tick += 1;
        self.frame.tick = self.tick;
        self.frame.rng = *rng;
        for (slot, (now, _)) in self.frame.health.iter_mut().zip(health) {
            *slot = f64::from(*now);
        }
        for (actor, &(now, max)) in health.iter().enumerate() {
            if max == 0 {
                continue;
            }
            let phase = ((max - now.min(max)) * PHASES / max) as u8;
            if phase > self.phases[actor] {
                self.phases[actor] = phase;
                self.fire(ON_PHASE, (actor as f64, f64::from(phase), f64::from(now)));
            }
        }
        if self.beat_every > 0 && self.tick.is_multiple_of(self.beat_every) {
            self.fire(ON_BEAT, (self.tick as f64, 0.0, 0.0));
        }
        *rng = self.frame.rng;
    }

    /// Runs one hook. A hook that doesn't end well leaves no commands, and that goes in the chain.
    fn fire(&mut self, hook: usize, args: (f64, f64, f64)) {
        let Some(function) = self.hooks[hook].as_ref() else { return };
        let before = alloc::vm_counts();
        self.frame.fault = false;
        let queued_before = self.frame.queued;
        let board_before = self.frame.board;
        let end = self.vm.fire(&mut self.frame, function, args);
        self.stats.commands_dropped += u64::from(std::mem::take(&mut self.frame.dropped));
        let code: u8 = match (&end, self.frame.fault) {
            (HookEnd::Returned, false) => 0,
            (HookEnd::Returned, true) => 1,
            (HookEnd::OverBudget, _) => 2,
            (HookEnd::Failed(_), _) => 3,
        };
        if code != 0 {
            self.stats.commands_discarded += (self.frame.queued - queued_before) as u64;
            self.frame.queued = queued_before;
            self.frame.board = board_before;
        }
        match end {
            HookEnd::Returned => {}
            HookEnd::OverBudget => self.stats.hooks_over_budget += 1,
            HookEnd::Failed(error) => {
                self.stats.hooks_failed += 1;
                self.error = Some(error);
            }
        }
        let mut hash = StateHasher::new();
        hash.u64(self.ends_hash);
        hash.u64(self.tick);
        hash.u8(hook as u8);
        hash.u8(code);
        self.ends_hash = hash.finish();
        let spent = alloc::vm_counts();
        let allocations = spent.allocations.wrapping_sub(before.allocations);
        self.stats.hooks_fired += 1;
        self.stats.script_allocations += allocations;
        self.stats.script_bytes += spent.bytes.wrapping_sub(before.bytes);
        self.stats.worst_hook_allocations = self.stats.worst_hook_allocations.max(allocations);
    }

    /// The one point commands apply: in order, each hashed by its bits.
    fn apply_commands(&mut self, slow_motion: &mut SlowMotion) {
        for index in 0..self.frame.queued {
            let command = self.frame.queue[index];
            let mut hash = StateHasher::new();
            hash.u64(self.commands_hash);
            hash.u64(self.tick);
            match command {
                Command::None => {}
                Command::SlowMotion { per_mille, steps } => {
                    // Refused here rather than in the binding, so a bad number can't reach time's rule.
                    if per_mille > 0 && 1_000_000 % per_mille == 0 {
                        slow_motion.request(per_mille, steps);
                    }
                    hash.u8(1);
                    hash.u32(per_mille);
                    hash.u16(steps);
                }
                Command::Mark(value) => {
                    hash.u8(2);
                    write_f64(&mut hash, value);
                }
            }
            self.commands_hash = hash.finish();
        }
        self.stats.commands_applied += self.frame.queued as u64;
        self.frame.queued = 0;
    }
}

/// A number by its bits, with any NaN as one, as the checksum writes floats.
fn write_f64(hash: &mut StateHasher, value: f64) {
    hash.u64(if value.is_nan() { f64::NAN.to_bits() } else { value.to_bits() });
}

/// Puts `host` in a simulation and adds the system that fires its hooks, last in the step, where
/// everything a hook reads has settled and a slow motion it asks for starts on the next.
pub fn attach(sim: &mut Simulation, host: ScriptHost) {
    sim.insert_resource(host);
    sim.add_combat_system(CombatSet::Cues, fire_hooks);
}

fn fire_hooks(
    table: Res<'_, ActorTable>,
    fighters: Query<'_, '_, &Fighter>,
    mut slow_motion: ResMut<'_, SlowMotion>,
    mut rng: ResMut<'_, SimRng>,
    mut host: ResMut<'_, ScriptHost>,
) {
    let mut health = [(0, 0); FRAME_ACTORS];
    for (slot, (_, entity)) in health.iter_mut().zip(table.iter()) {
        if let Ok(fighter) = fighters.get(entity) {
            *slot = (fighter.health(), fighter.max_health());
        }
    }
    host.run(&health, &mut rng.0);
    host.apply_commands(&mut slow_motion);
}

/// The host [`attach`] put in `sim`, to read: its stats, its digest, a collection step between steps.
///
/// # Panics
///
/// If nothing's been attached.
#[must_use]
pub fn host_of(sim: &Simulation) -> &ScriptHost {
    sim.world().resource::<ScriptHost>()
}

/// The host [`attach`] put in `sim`, to change between steps: a reload.
///
/// # Panics
///
/// If nothing's been attached.
pub fn host_of_mut(sim: &mut Simulation) -> Mut<'_, ScriptHost> {
    sim.resource_mut::<ScriptHost>()
}
