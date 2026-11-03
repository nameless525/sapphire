//! Luau in the simulation (spec §5.24, `docs/m3-plan.md` §3): one sandboxed VM per simulation, firing
//! event hooks inside the fixed step, whose only outputs are typed commands and a fixed board of numbers.
//!
//! Owns:
//!
//! - [`ScriptVm`]: the VM. An allowlist of libraries, a prelude, frozen globals, a budget counted in
//!   interrupts and ended by a yield, a memory limit, a collector that only runs when asked.
//! - [`cook`] and [`scan`]: compiling at one pinned level with libm's builtins off, and refusing bytes that
//!   reach the C library, wherever they came from.
//! - [`bindings`]: what a script can call, as Rust that never raises and sends what it allocates to gate 4.
//! - [`ScriptHost`]: which hooks are bound, what lasts between them, and the system that fires them.
//!
//! Doesn't own: anything of gameplay's. It depends on `sapphire_gameplay` and not the other way round,
//! and adds itself with `Simulation::add_combat_system`.

pub mod bindings;
pub mod compile;
pub mod host;
pub mod scan;
pub mod vm;

pub use bindings::{BOARD_SLOTS, Binding, Command, Frame};
pub use compile::{CookError, cook};
pub use host::{HOOKS, ScriptHost, ScriptStats, attach, host_of, host_of_mut};
pub use vm::{HookEnd, Limits, ScriptVm, VmError};
