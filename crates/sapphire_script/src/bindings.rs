//! What a script can call: queries that read a [`Frame`], commands that go into its queue, and pinned
//! math, all of it Rust that never raises.
//!
//! These are `lua_CFunction`s rather than `Lua::create_function` closures for what they don't do: a
//! closure returning `Err`, or one handed an argument `FromLua` refuses, reaches `lua_error` from a Rust
//! frame, and that C++ exception ends the process under `panic = "abort"`. A wrong argument here sets
//! the frame's fault, the hook goes on with a zero, and the hook's commands are thrown away when it
//! ends, on every run alike. L2 generates them from a declared table; these are the shape.

use std::cell::Cell;
use std::ffi::c_int;
use std::ptr;

use mlua::{Lua, Table, ffi, lua_State};
use sapphire_core::alloc::HostAllocations;
use sapphire_math::{Pcg32, det};

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
    /// The simulation's randomness, for the hook's length.
    pub rng: *mut Pcg32,
    /// A binding was handed something it can't use: this hook's commands don't count.
    pub fault: bool,
}

impl Frame {
    /// A frame with nothing asked for.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            tick: 0,
            health: [0.0; FRAME_ACTORS],
            board: [0.0; BOARD_SLOTS],
            queue: [Command::None; QUEUE_LEN],
            queued: 0,
            dropped: 0,
            rng: ptr::null_mut(),
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

// SAFETY: `rng`, the one thing here that isn't `Send`, is null except while a hook runs, and a hook runs
// to its end on the thread that set it.
unsafe impl Send for Frame {}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}

thread_local! {
    static FRAME: Cell<*mut Frame> = const { Cell::new(ptr::null_mut()) };
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

/// The running hook's frame, with allocations going back to the thread's count, so a Rust allocation
/// in a binding still fails gate 4. What every binding goes through, a test's allocating one included.
/// Outside a hook, which can't happen, it answers `None`.
pub fn with_frame<R>(body: impl FnOnce(&mut Frame) -> R) -> Option<R> {
    let _host = HostAllocations::enter();
    let frame = FRAME.get();
    // SAFETY: `enter` set this from a `&mut Frame` the hook's caller holds until the guard drops, and a
    // hook runs on this thread with nothing else reading it: no binding holds this past its return.
    unsafe { frame.as_mut() }.map(body)
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
                ffi::lua_pushnumber(state, f64::from($function(value)));
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
        ffi::lua_pushnumber(state, f64::from(det::atan2(y, x)));
    }
    1
}

unsafe extern "C-unwind" fn pow(state: *mut lua_State) -> c_int {
    // SAFETY: as `unary!`'s.
    unsafe {
        let (base, exponent) = (number_or_nan(state, 1), number_or_nan(state, 2));
        ffi::lua_pushnumber(state, f64::from(det::pow(base, exponent)));
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
    let value = with_frame(|frame| {
        // SAFETY: the system that fires the hook set this from its `ResMut<SimRng>`, which it holds
        // until the hook ends.
        unsafe { frame.rng.as_mut() }.map_or(0.0, |rng| f64::from(rng.unit_f32()))
    });
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

/// Registers a binding, for a test's or L2's generated table: goes in `sapphire`, before the sandbox.
pub fn add(lua: &Lua, table: &Table, name: &str, function: ffi::lua_CFunction) -> mlua::Result<()> {
    // SAFETY: every function passed here never raises, which is what this module holds them to; a
    // `lua_CFunction` that did would be a C++ exception through a Rust frame.
    let function = unsafe { lua.create_c_function(function) }?;
    table.raw_set(name, function)
}

/// The pinned math and the `sapphire` table, on a state whose sandbox hasn't closed.
pub(crate) fn install(lua: &Lua) -> mlua::Result<()> {
    let math: Table = lua.globals().get("math")?;
    for (name, function) in [
        ("sin", sin as ffi::lua_CFunction),
        ("cos", cos),
        ("tan", tan),
        ("asin", asin),
        ("acos", acos),
        ("atan", atan),
        ("atan2", atan2),
        ("exp", exp),
        ("log", log),
        ("pow", pow),
    ] {
        add(lua, &math, name, function)?;
    }
    let sapphire = lua.create_table()?;
    for (name, function) in [
        ("tick", tick as ffi::lua_CFunction),
        ("health", health),
        ("random", random),
        ("board", board),
        ("set_board", set_board),
        ("slow_motion", slow_motion),
        ("mark", mark),
    ] {
        add(lua, &sapphire, name, function)?;
    }
    lua.globals().raw_set("sapphire", sapphire)
}
