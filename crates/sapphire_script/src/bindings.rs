//! What a script can call: queries that read a [`HookFrame`], commands that go into its queue, and pinned
//! math, all of it Rust that never raises.
//!
//! These are `lua_CFunction`s rather than `Lua::create_function` closures for what they don't do: a
//! closure returning `Err`, or one handed an argument `FromLua` refuses, reaches `lua_error` from a Rust
//! frame, and that C++ exception ends the process under `panic = "abort"`. A wrong argument here sets
//! the frame's fault, the hook goes on with a zero, and the hook's commands are thrown away when it
//! ends, on every run alike.
//!
//! **The rules for a binding,** which `generated.rs`'s follow by construction, being made from the one
//! table in `api.toml`, and which only a test's own or the pinned math below has to be held to:
//!
//! - It's a [`Binding`] of a raw `lua_CFunction`. Nothing here takes a `Lua::create_function`.
//! - It takes and pushes only numbers. A number needs no allocation, so the memory limit can't raise.
//! - Its work goes in [`with_frame`], or [`in_host`] if it needs no frame. That sends a Rust allocation
//!   back to the thread's count, so it fails gate 4. [`crate::ScriptVm::new`] calls every binding once, with
//!   no frame and no arguments, and refuses one that didn't go through: with the VM's count diverted, an
//!   allocation in that binding would pass the gate unseen. That call can't see a path it didn't take, which
//!   is why the table's wrappers have only one.

use std::cell::Cell;
use std::ffi::c_int;
use std::ptr;

use mlua::{Function, Lua, Table, ffi, lua_State};
use sapphire_core::alloc::HostAllocations;
use sapphire_gameplay::HookFrame;
use sapphire_math::det;

use crate::vm::VmError;

#[rustfmt::skip]
mod generated;

pub use generated::{API, HOOK_COUNT, HOOKS, ON_BEAT, ON_DEFEAT, ON_HIT, ON_MOVE, ON_PHASE};

thread_local! {
    static FRAME: Cell<*mut HookFrame> = const { Cell::new(ptr::null_mut()) };
    /// How many times [`in_host`] has been entered: what the check at a VM's start reads.
    static HOST_ENTRIES: Cell<u64> = const { Cell::new(0) };
}

/// Puts a frame where the bindings find it, until this is dropped.
pub(crate) struct FrameGuard(*mut HookFrame);

pub(crate) fn enter(frame: &mut HookFrame) -> FrameGuard {
    FrameGuard(FRAME.replace(frame))
}

/// Initializers and startup validation must not inherit a calling hook's frame.
pub(crate) fn without_frame() -> FrameGuard {
    FrameGuard(FRAME.replace(ptr::null_mut()))
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

/// [`in_host`], with exclusive access to the running hook's frame. Outside a hook, or during another
/// `with_frame` call on that frame, it answers `None` without running `body`. A nested VM may install
/// its own frame; when it returns, the outer frame remains borrowed until `body` returns.
pub fn with_frame<R>(body: impl FnOnce(&mut HookFrame) -> R) -> Option<R> {
    in_host(|| {
        let guard = without_frame();
        // SAFETY: `enter` set this from a live exclusive borrow. Removing the pointer before making
        // the reference refuses reentry, and the guard restores it only after `body` returns/unwinds.
        unsafe { guard.0.as_mut() }.map(body)
    })
}

/// A number argument, or a fault and zero.
pub(crate) unsafe fn number(state: *mut lua_State, index: c_int, frame: &mut HookFrame) -> f64 {
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

/// One function a script can call: a row of the table L2 generates.
///
/// ```compile_fail
/// fn unchecked(function: mlua::ffi::lua_CFunction) {
///     let _ = sapphire_script::Binding { name: "unchecked", function };
/// }
/// ```
///
/// ```compile_fail
/// fn unchecked(function: mlua::ffi::lua_CFunction) {
///     let _ = sapphire_script::Binding::new("unchecked", function);
/// }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Binding {
    /// What a script calls it.
    pub name: &'static str,
    function: ffi::lua_CFunction,
}

impl Binding {
    /// Registers a raw callback implementation. Rust work goes through [`with_frame`] or [`in_host`];
    /// startup probes that participation, but cannot prove the callback's safety for every input.
    ///
    /// # Safety
    ///
    /// `function` must accept any live Luau state supplied by the VM, including startup's no-frame,
    /// no-argument probe. It must validate argument types before access, preserve Lua stack/value
    /// invariants, and return the correct number of initialized results. It must never raise a Lua
    /// error, yield across a Rust frame, or let a panic/foreign exception cross the callback boundary.
    /// Any binary code it executes must be complete, unmodified, structurally valid output of the
    /// exact linked compiler; it must preserve the VM's sandbox and execution invariants.
    #[must_use]
    pub const unsafe fn new(name: &'static str, function: ffi::lua_CFunction) -> Self {
        Self { name, function }
    }
}

/// A row of the declared table: a [`Binding`], and what `sapphire.d.luau` must say of it.
#[derive(Clone, Copy, Debug)]
pub struct Declared {
    pub binding: Binding,
    pub args: &'static [&'static str],
    pub returns: &'static [&'static str],
}

const fn binding(name: &'static str, function: ffi::lua_CFunction) -> Binding {
    // SAFETY: these rows contain only this module's validated numeric, nonraising callbacks.
    unsafe { Binding::new(name, function) }
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
    for row in API.iter().map(|declared| &declared.binding).chain(more) {
        add(lua, &sapphire, row)?;
    }
    lua.globals().raw_set("sapphire", sapphire)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_access_is_exclusive_and_restored_after_unwind() {
        assert!(with_frame(|_| panic!("no frame")).is_none());
        let mut frame = HookFrame::new(0);
        {
            let _entered = enter(&mut frame);
            with_frame(|outer| {
                outer.board[0] = 7.0;
                assert!(with_frame(|_| panic!("already borrowed")).is_none());
                outer.board[0] += 1.0;
            })
            .unwrap();
            let panic = std::panic::catch_unwind(|| {
                with_frame(|_| panic!("a host callback unwinds"));
            });
            assert!(panic.is_err());
            assert_eq!(with_frame(|frame| frame.board[0]), Some(8.0));
        }
        assert!(with_frame(|_| ()).is_none());
        assert_eq!(frame.board[0], 8.0);
    }
}
