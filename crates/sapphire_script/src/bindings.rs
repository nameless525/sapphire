//! What a script can call: queries that read a [`Frame`], commands that go into its queue, and pinned
//! math, all of it Rust that never raises.
//!
//! These are `lua_CFunction`s rather than `Lua::create_function` closures for what they don't do: a
//! closure returning `Err`, or one handed an argument `FromLua` refuses, reaches `lua_error` from a Rust
//! frame, and that C++ exception ends the process under `panic = "abort"`. A wrong argument here sets
//! the frame's fault, the hook goes on with a zero, and the hook's commands are thrown away when it
//! ends, on every run alike.
//!
//! **The rules for a binding,** which L2's generated ones follow by construction:
//!
//! - It's a [`Binding`] of a raw `lua_CFunction`. Nothing here takes a `Lua::create_function`.
//! - It takes and pushes only numbers. A number needs no allocation, so the memory limit can't raise.
//! - Its work goes in [`with_frame`], or [`in_host`] if it needs no frame. That sends a Rust allocation
//!   back to the thread's count, so it fails gate 4. [`crate::ScriptVm::new`] calls every binding once, with
//!   no frame and no arguments, and refuses one that didn't go through: with the VM's count diverted, an
//!   allocation in that binding would pass the gate unseen.

use std::cell::Cell;
use std::ffi::c_int;
use std::ptr;

use mlua::{Function, Lua, Table, ffi, lua_State};
use sapphire_core::alloc::HostAllocations;
use sapphire_math::{Pcg32, det};

use crate::vm::VmError;

/// Numbers that last between hooks: the only state a script has (spec §5.24).
pub const BOARD_SLOTS: usize = 64;

/// Commands one hook may leave.
pub const QUEUE_LEN: usize = 16;

/// Actors a query can see.
pub const FRAME_ACTORS: usize = 16;

/// What a hook asked for, applied at one point in the step, and hashed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Command {
    #[default]
    None,
    /// The world's clock runs at this rate for this many steps.
    SlowMotion { per_mille: u32, steps: u16 },
    /// Nothing but a number in the checksum: what a test hook leaves to show it ran.
    Mark(f64),
}

/// What the bindings of the running hook read and write. Nothing here points past the hook.
pub struct Frame {
    /// The simulation's tick.
    pub tick: u64,
    /// Each actor's health, by the order the system saw them; a query outside it reads 0.
    pub health: [f64; FRAME_ACTORS],
    /// The simulation-owned numbers that last.
    pub board: [f64; BOARD_SLOTS],
    /// What this hook has asked for.
    pub queue: [Command; QUEUE_LEN],
    /// How much of `queue` is in use.
    pub queued: usize,
    /// Commands past the end of `queue`.
    pub dropped: u32,
    /// The simulation's randomness: copied in before a step's hooks and back after them.
    pub rng: Pcg32,
    /// A binding was handed something it can't use: this hook's commands don't count.
    pub fault: bool,
}

impl Frame {
    /// A frame with nothing asked for.
    #[must_use]
    pub fn new() -> Self {
        Self {
            tick: 0,
            health: [0.0; FRAME_ACTORS],
            board: [0.0; BOARD_SLOTS],
            queue: [Command::None; QUEUE_LEN],
            queued: 0,
            dropped: 0,
            rng: Pcg32::new(0, 0),
            fault: false,
        }
    }

    fn push(&mut self, command: Command) {
        if let Some(slot) = self.queue.get_mut(self.queued) {
            *slot = command;
            self.queued += 1;
        } else {
            self.dropped += 1;
        }
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static FRAME: Cell<*mut Frame> = const { Cell::new(ptr::null_mut()) };
    /// How many times [`in_host`] has been entered: what the check at a VM's start reads.
    static HOST_ENTRIES: Cell<u64> = const { Cell::new(0) };
}

/// Puts a frame where the bindings find it, until this is dropped.
pub(crate) struct FrameGuard(*mut Frame);

pub(crate) fn enter(frame: &mut Frame) -> FrameGuard {
    FrameGuard(FRAME.replace(frame))
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        FRAME.set(self.0);
    }
}

/// Runs a binding's work with allocations going back to the thread's count, so one fails gate 4.
pub fn in_host<R>(body: impl FnOnce() -> R) -> R {
    HOST_ENTRIES.set(HOST_ENTRIES.get().wrapping_add(1));
    let _host = HostAllocations::enter();
    body()
}

/// [`in_host`], with the running hook's frame. Outside a hook, which only the check at a VM's start
/// does, it answers `None` without running `body`.
pub fn with_frame<R>(body: impl FnOnce(&mut Frame) -> R) -> Option<R> {
    in_host(|| {
        let frame = FRAME.get();
        // SAFETY: `enter` set this from a `&mut Frame` the hook's caller holds until the guard drops, and
        // a hook runs on this thread with nothing else reading it: no binding holds this past its return.
        unsafe { frame.as_mut() }.map(body)
    })
}

/// A number argument, or a fault and zero.
unsafe fn number(state: *mut lua_State, index: c_int, frame: &mut Frame) -> f64 {
    // SAFETY: the caller passes a live state, and neither call raises.
    unsafe {
        if ffi::lua_type(state, index) == ffi::LUA_TNUMBER {
            ffi::lua_tonumberx(state, index, ptr::null_mut())
        } else {
            frame.fault = true;
            0.0
        }
    }
}

/// A number argument, or NaN: `lua_tonumberx` alone would read text through the C library's `strtod`.
unsafe fn number_or_nan(state: *mut lua_State, index: c_int) -> f32 {
    // SAFETY: the caller passes a live state, and neither call raises.
    unsafe {
        if ffi::lua_type(state, index) == ffi::LUA_TNUMBER {
            ffi::lua_tonumberx(state, index, ptr::null_mut()) as f32
        } else {
            f32::NAN
        }
    }
}

/// A one-number function of `sapphire_math::det`, through `f32`: scripts' numbers are `f64`, and what's
/// pinned is `f32`. A wrong argument reads as NaN, which any comparison then shows.
macro_rules! unary {
    ($name:ident, $function:path) => {
        unsafe extern "C-unwind" fn $name(state: *mut lua_State) -> c_int {
            // SAFETY: Luau passes a live state and a slot for one result, and neither call raises.
            unsafe {
                let value = number_or_nan(state, 1);
                ffi::lua_pushnumber(state, f64::from(in_host(|| $function(value))));
            }
            1
        }
    };
}

unary!(sin, det::sin);
unary!(cos, det::cos);
unary!(tan, det::tan);
unary!(asin, det::asin);
unary!(acos, det::acos);
unary!(atan, det::atan);
unary!(exp, det::exp);
unary!(log, det::ln);

unsafe extern "C-unwind" fn atan2(state: *mut lua_State) -> c_int {
    // SAFETY: as `unary!`'s.
    unsafe {
        let (y, x) = (number_or_nan(state, 1), number_or_nan(state, 2));
        ffi::lua_pushnumber(state, f64::from(in_host(|| det::atan2(y, x))));
    }
    1
}

unsafe extern "C-unwind" fn pow(state: *mut lua_State) -> c_int {
    // SAFETY: as `unary!`'s.
    unsafe {
        let (base, exponent) = (number_or_nan(state, 1), number_or_nan(state, 2));
        ffi::lua_pushnumber(state, f64::from(in_host(|| det::pow(base, exponent))));
    }
    1
}

/// `sapphire.tick()`: the simulation's tick.
unsafe extern "C-unwind" fn tick(state: *mut lua_State) -> c_int {
    let tick = with_frame(|frame| frame.tick).unwrap_or(0);
    // SAFETY: Luau passes a live state and a slot for one result.
    unsafe { ffi::lua_pushnumber(state, tick as f64) };
    1
}

/// `sapphire.health(index)`: an actor's health.
unsafe extern "C-unwind" fn health(state: *mut lua_State) -> c_int {
    let value = with_frame(|frame| {
        // SAFETY: Luau passes a live state.
        let index = unsafe { number(state, 1, frame) };
        frame.health.get(index as usize).copied().unwrap_or(0.0)
    });
    // SAFETY: Luau passes a live state and a slot for one result.
    unsafe { ffi::lua_pushnumber(state, value.unwrap_or(0.0)) };
    1
}

/// `sapphire.random()`: the next of the simulation's numbers in `0..1`, so it's in the checksum.
unsafe extern "C-unwind" fn random(state: *mut lua_State) -> c_int {
    let value = with_frame(|frame| f64::from(frame.rng.unit_f32()));
    // SAFETY: Luau passes a live state and a slot for one result.
    unsafe { ffi::lua_pushnumber(state, value.unwrap_or(0.0)) };
    1
}

/// `sapphire.board(slot)`: a number that lasts.
unsafe extern "C-unwind" fn board(state: *mut lua_State) -> c_int {
    let value = with_frame(|frame| {
        // SAFETY: Luau passes a live state.
        let slot = unsafe { number(state, 1, frame) };
        frame.board.get(slot as usize).copied().unwrap_or_else(|| {
            frame.fault = true;
            0.0
        })
    });
    // SAFETY: Luau passes a live state and a slot for one result.
    unsafe { ffi::lua_pushnumber(state, value.unwrap_or(0.0)) };
    1
}

/// `sapphire.set_board(slot, value)`.
unsafe extern "C-unwind" fn set_board(state: *mut lua_State) -> c_int {
    with_frame(|frame| {
        // SAFETY: Luau passes a live state.
        let (slot, value) = unsafe { (number(state, 1, frame), number(state, 2, frame)) };
        match frame.board.get_mut(slot as usize) {
            Some(cell) => *cell = value,
            None => frame.fault = true,
        }
    });
    0
}

/// `sapphire.slow_motion(per_mille, steps)`: a command.
unsafe extern "C-unwind" fn slow_motion(state: *mut lua_State) -> c_int {
    with_frame(|frame| {
        // SAFETY: Luau passes a live state.
        let (per_mille, steps) = unsafe { (number(state, 1, frame), number(state, 2, frame)) };
        frame.push(Command::SlowMotion { per_mille: per_mille as u32, steps: steps as u16 });
    });
    0
}

/// `sapphire.mark(value)`: a command with no effect but its hash.
unsafe extern "C-unwind" fn mark(state: *mut lua_State) -> c_int {
    with_frame(|frame| {
        // SAFETY: Luau passes a live state.
        let value = unsafe { number(state, 1, frame) };
        frame.push(Command::Mark(value));
    });
    0
}

/// One function a script can call: a row of the table L2 generates.
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    /// What a script calls it.
    pub name: &'static str,
    /// It never raises, and it goes through [`with_frame`] or [`in_host`].
    pub function: ffi::lua_CFunction,
}

const fn binding(name: &'static str, function: ffi::lua_CFunction) -> Binding {
    Binding { name, function }
}

/// The pinned math, in `math`.
const MATH: &[Binding] = &[
    binding("sin", sin),
    binding("cos", cos),
    binding("tan", tan),
    binding("asin", asin),
    binding("acos", acos),
    binding("atan", atan),
    binding("atan2", atan2),
    binding("exp", exp),
    binding("log", log),
    binding("pow", pow),
];

/// What the engine offers, in `sapphire`.
const SAPPHIRE: &[Binding] = &[
    binding("tick", tick),
    binding("health", health),
    binding("random", random),
    binding("board", board),
    binding("set_board", set_board),
    binding("slow_motion", slow_motion),
    binding("mark", mark),
];

fn add(lua: &Lua, table: &Table, row: &Binding) -> Result<(), VmError> {
    // SAFETY: every function in a `Binding` never raises, which its type's docs hold it to. One that did
    // would be a C++ exception through a Rust frame.
    let function: Function = unsafe { lua.create_c_function(row.function) }?;
    let before = HOST_ENTRIES.get();
    let () = function.call::<mlua::MultiValue>(()).map(drop)?;
    if HOST_ENTRIES.get() == before {
        return Err(VmError::UnflippedBinding(row.name));
    }
    table.raw_set(row.name, function)?;
    Ok(())
}

/// The pinned math and the `sapphire` table with `more` in it, on a state whose sandbox hasn't closed.
pub(crate) fn install(lua: &Lua, more: &[Binding]) -> Result<(), VmError> {
    let math: Table = lua.globals().get("math")?;
    for row in MATH {
        add(lua, &math, row)?;
    }
    let sapphire = lua.create_table()?;
    for row in SAPPHIRE.iter().chain(more) {
        add(lua, &sapphire, row)?;
    }
    lua.globals().raw_set("sapphire", sapphire)?;
    Ok(())
}
