//! Luau in the simulation (spec §5.24, `docs/m3-plan.md` §3): one sandboxed VM per simulation, firing
//! event hooks inside the fixed step, whose only outputs are typed commands and a fixed board of numbers.
//!
//! Owns:
//!
//! - [`ScriptVm`]: the VM. An allowlist of libraries, a prelude, frozen globals, a budget counted in
//!   interrupts and ended by a yield, a memory limit, a collector that only runs when asked.
//! - [`bindings`]: what a script can call, as Rust that never raises and sends what it allocates to gate 4,
//!   made by `cargo xtask script-api` from the table in `api.toml`, which makes `sapphire.d.luau` too.
//! - [`ScriptHost`]: which hooks are bound, and the system that fires them; and a dialogue's conditions
//!   ([`ConditionSet`]), bound in the same VM and asked between steps on a read-only frame ([`ask`]).
//! - [`scenarios`]: replay scenarios with a script in them.
//!
//! Doesn't own: compiling and scanning, which are `sapphire_bytecode`'s, for the cooker to link without a
//! simulation (re-exported here as [`cook`] and [`scan`]); or what a script reads and asks for, which is
//! `sapphire_gameplay::script`'s. It depends on `sapphire_gameplay` and not the other way round, and adds
//! itself with `Simulation::add_combat_system`.

mod api;
pub mod bindings;
pub mod host;
pub mod scenarios;
pub mod vm;

pub use bindings::{API, Binding, Declared, HOOKS};
pub use host::{
    ConditionSet, ReloadError, Reloaded, ScriptHost, ScriptStats, ask, attach, host_of, host_of_mut, rebind,
    reload,
};
pub use sapphire_bytecode::compile::{self, CookedScript, cook};
pub use sapphire_bytecode::{CookError, scan};
pub use vm::{Hook, HookArgs, HookEnd, HookTable, Limits, ScriptVm, VmError};
