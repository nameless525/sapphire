//! The VM: Luau with an allowlist of libraries, read-only, on a count rather than a clock.

use std::cell::Cell;
use std::ffi::c_int;

use mlua::chunk::ChunkMode;
use mlua::state::{GcIncParams, GcMode};
use mlua::thread::ThreadStatus;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread, Value, ffi, lua_State};
use sapphire_core::alloc::VmAllocations;

use crate::bindings::{self, Binding, Frame};
use crate::compile::{self, CookError};

/// Globals a script may reach. Anything else, now or in a later Luau, is gone before the sandbox closes.
const GLOBALS: &[&str] = &[
    "assert", "bit32", "error", "ipairs", "math", "pairs", "rawequal", "rawget", "rawlen", "rawset",
    "select", "string", "table", "tonumber", "tostring", "type", "typeof", "vector", "sapphire",
];

/// What stays of each library. `math`'s transcendentals here are `sapphire_math`'s, put in by [`bindings`].
const LIBRARIES: &[(&str, &[&str])] = &[
    (
        "math",
        &[
            "abs", "acos", "asin", "atan", "atan2", "ceil", "clamp", "cos", "deg", "exp", "floor", "huge",
            "isfinite", "isinf", "isnan", "lerp", "log", "max", "min", "pi", "pow", "rad", "round", "sign",
            "sin", "sqrt", "tan",
        ],
    ),
    // Strings are only what a chunk spells: with no `..` (the scan's) and nothing here that builds one,
    // none gets long enough for a loop over it to outrun the budget, which counts interrupts and not
    // bytes. No `format`, whose float conversions are the C library's, no patterns, which backtrack
    // with no interrupt, and no `lower` or `upper`, which go by the locale.
    ("string", &["len", "sub"]),
    // `find`, `insert`, `remove` and `sort` are the prelude's, whose loops the budget counts: C's walk a
    // whole table on one interrupt. What's gone did the same, or (`unpack`) put a table on the stack.
    ("table", &["create", "find", "freeze", "insert", "isfrozen", "remove", "sort"]),
    (
        "vector",
        &[
            "abs",
            "ceil",
            "clamp",
            "create",
            "cross",
            "dot",
            "floor",
            "lerp",
            "magnitude",
            "max",
            "min",
            "normalize",
            "one",
            "sign",
            "zero",
        ],
    ),
];

const PRELUDE: &str = include_str!("prelude.luau");

/// How much a hook may do.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Bytes the VM may hold. Hitting it ends the hook that hit it.
    pub memory: usize,
    /// Calls and loop back-edges one hook may make before it's ended. It's a count, so a budget ends a
    /// hook at the same instruction on every run.
    pub interrupts: u64,
    /// Work in a collector's step, in Luau's KB: about 3 µs at 16. It bounds a step but for one thing: a
    /// table is marked whole, so a table of 130,000 slots is a 360 µs step at any size. The memory limit
    /// is what keeps a table from getting there.
    pub gc_step_kb: i32,
}

impl Default for Limits {
    /// Read from a Mac: at 10,000 interrupts a spinning hook ends in 25 µs, or 90 µs through bindings, and
    /// a real hook takes tens. 2 MiB holds the VM's own 520 KB and a boss's garbage between steps, and at
    /// 16 KB a step a 2 MiB heap's cycle takes about 50 steps.
    fn default() -> Self {
        Self { memory: 2 << 20, interrupts: 10_000, gc_step_kb: 16 }
    }
}

/// How a hook ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookEnd {
    /// It returned.
    Returned,
    /// It ran out of budget, and was ended at an interrupt with no error raised.
    OverBudget,
    /// It raised an error, a memory limit's included.
    Failed(String),
}

/// Why a VM can't start.
#[derive(Debug, thiserror::Error)]
pub enum VmError {
    #[error(transparent)]
    Lua(#[from] mlua::Error),
    #[error(transparent)]
    Cook(#[from] CookError),
    #[error("a script's chunk must return a table of hooks")]
    NotAHookTable,
    #[error("the binding `{0}` didn't go through `with_frame`, so what it allocates wouldn't reach gate 4")]
    UnflippedBinding(&'static str),
}

thread_local! {
    /// Interrupts the running hook has left. A count kept here because the interrupt sees only a state.
    static BUDGET: Cell<u64> = const { Cell::new(0) };
    /// Interrupts since the VM started on this thread: how a budget's read.
    static INTERRUPTS: Cell<u64> = const { Cell::new(0) };
}

/// Spends one of the hook's budget, and ends it by a yield when that runs out. Set directly, rather
/// than through `Lua::set_interrupt`, for what it leaves out: `mlua`'s proc wraps this in a catch, a
/// clone of the callback and a checked failure slot on every back-edge. Raises nothing, so it's safe
/// under `panic = "abort"`. `lua_yield` raises when it can't yield, so that's asked first; a script has
/// nothing left that calls back across a C boundary, which is the only way not to be able.
unsafe extern "C-unwind" fn interrupt(state: *mut lua_State, gc: c_int) {
    if gc >= 0 {
        return;
    }
    INTERRUPTS.set(INTERRUPTS.get().wrapping_add(1));
    let left = BUDGET.get();
    if left > 0 {
        BUDGET.set(left - 1);
        return;
    }
    // SAFETY: Luau passes a live state, and asking first means `lua_yield` can't raise.
    unsafe {
        if ffi::lua_isyieldable(state) != 0 {
            ffi::lua_yield(state, 0);
        }
    }
}

/// One simulation's VM.
pub struct ScriptVm {
    lua: Lua,
    /// Runs every hook, so a budget has somewhere to yield to.
    thread: Thread,
    limits: Limits,
}

impl ScriptVm {
    /// A sandboxed VM with nothing loaded, the one way to make one: the allowlist applied, the prelude
    /// run, the globals frozen, the budget's interrupt set, the memory limit on and the collector
    /// stopped, so it runs only when [`Self::gc_step`] says.
    pub fn new(limits: Limits) -> Result<Self, VmError> {
        Self::with_bindings(limits, &[])
    }

    /// [`Self::new`], with more bindings put in `sapphire` before the sandbox closes: the shape L2's
    /// generated table takes, and how a test adds one that misbehaves. Each is called once here, with no
    /// frame and no arguments, and one that doesn't go through [`bindings::with_frame`] is refused.
    pub fn with_bindings(limits: Limits, more: &[Binding]) -> Result<Self, VmError> {
        let lua = Lua::new_with(
            StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::BIT | StdLib::VECTOR,
            LuaOptions::new(),
        )?;
        bindings::install(&lua, more)?;
        let prelude = compile::compile_unchecked(PRELUDE, compile::SHIPPING_LEVEL)?;
        lua.load(prelude).set_name("=prelude").set_mode(ChunkMode::Binary).exec()?;
        keep_only(&lua.globals(), GLOBALS)?;
        for (library, keep) in LIBRARIES {
            keep_only(&lua.globals().get::<Table>(*library)?, keep)?;
        }
        lua.sandbox(true)?;
        lua.globals().set_readonly(true);
        // SAFETY: the callback is only a function pointer, and it's on the VM's global state, which every
        // thread shares. `mlua` doesn't read this slot unless `set_interrupt` is called, and that isn't.
        unsafe {
            lua.exec_raw::<()>((), |state: *mut lua_State| {
                (*ffi::lua_callbacks(state)).interrupt = Some(interrupt);
            })?;
        }
        lua.set_memory_limit(limits.memory)?;
        lua.gc_set_mode(GcMode::Incremental(GcIncParams::default().step_size(limits.gc_step_kb)));
        lua.gc_stop();
        let nothing = lua.create_function(|_, ()| Ok(()))?;
        let thread = lua.create_thread(nothing)?;
        Ok(Self { lua, thread, limits })
    }

    /// The Luau state, for a test to look behind the sandbox. Anything else goes through this type: what
    /// this hands out can make a `create_function` binding or run a chunk the scan hasn't read.
    #[doc(hidden)]
    #[must_use]
    pub const fn lua_for_tests(&self) -> &Lua {
        &self.lua
    }

    /// Whether the collector is stopped, as it should be whenever a step isn't asking it for work.
    #[must_use]
    pub fn collector_stopped(&self) -> bool {
        !self.lua.gc_is_running()
    }

    /// Runs a cooked chunk, checked again here since a pushed chunk's bytes came from elsewhere, and
    /// returns the table of hooks it makes. Allocates, so it happens between steps.
    pub fn load(&self, cooked: &[u8], name: &str) -> Result<Table, VmError> {
        compile::check(cooked)?;
        let _vm = VmAllocations::enter();
        // A chunk's top level gets the same budget as a hook.
        BUDGET.set(self.limits.interrupts);
        let value: Value = self.lua.load(cooked).set_name(name).set_mode(ChunkMode::Binary).eval()?;
        let Value::Table(hooks) = value else { return Err(VmError::NotAHookTable) };
        freeze_reachable(&self.lua, &hooks)?;
        Ok(hooks)
    }

    /// Runs one hook to its end, with `frame` for its bindings to read and write.
    pub fn fire(&self, frame: &mut Frame, hook: &Function, args: impl mlua::IntoLuaMulti) -> HookEnd {
        let _vm = VmAllocations::enter();
        let _frame = bindings::enter(frame);
        BUDGET.set(self.limits.interrupts);
        if let Err(error) = self.thread.reset(hook.clone()) {
            return HookEnd::Failed(error.to_string());
        }
        match self.thread.resume::<()>(args) {
            Err(error) => HookEnd::Failed(error.to_string()),
            Ok(()) if matches!(self.thread.status(), ThreadStatus::Finished) => HookEnd::Returned,
            Ok(()) => {
                // A yielded thread holds its stack, and all a hook's garbage with it, until the next hook.
                let _ = self.thread.reset(hook.clone());
                HookEnd::OverBudget
            }
        }
    }

    /// What the running count of interrupts has reached on this thread.
    #[must_use]
    pub fn interrupts() -> u64 {
        INTERRUPTS.get()
    }

    /// Does one bounded piece of collection, and stops the collector again. Returns whether a cycle ended.
    ///
    /// Both go in one breath: `LUA_GCSTEP` restarts a stopped collector and leaves it deep in debt, since
    /// it reckons its credit from the stop's threshold. Any allocation before the stop would then run an
    /// assist as long as the debt, which is what `Lua::gc_step` plus `Lua::gc_stop` does now and then. The
    /// limit comes off because shrinking a stack or the strings' table allocates first, and a memory error
    /// from here would cross this Rust frame.
    pub fn gc_step(&self) -> Result<bool, VmError> {
        let limit = self.lua.set_memory_limit(0)?;
        let finished = Cell::new(false);
        // SAFETY: `exec_raw` hands over a live state, and with no limit neither call raises.
        let stepped = unsafe {
            self.lua.exec_raw::<()>((), |state: *mut lua_State| {
                finished.set(ffi::lua_gc(state, ffi::LUA_GCSTEP, 0) != 0);
                ffi::lua_gc(state, ffi::LUA_GCSTOP, 0);
            })
        };
        self.lua.set_memory_limit(limit)?;
        stepped?;
        Ok(finished.get())
    }

    /// Bytes the VM holds.
    #[must_use]
    pub fn used_memory(&self) -> usize {
        self.lua.used_memory()
    }
}

/// Freezes every table a chunk's hooks can reach through its table and their upvalues, so nothing a hook
/// writes outlives it but what's on the board: with `SETUPVAL` refused by the scan and the globals frozen,
/// that's every place left. A hook that tries raises, in Luau's frames, on every run alike.
fn freeze_reachable(lua: &Lua, hooks: &Table) -> mlua::Result<()> {
    // SAFETY: `exec_raw` hands over a live state with `hooks` on top. Nothing below can raise on it: each
    // call is given a value of the type it wants and checks the stack first, and `lua_next` only ever
    // sees a key it returned.
    unsafe {
        lua.exec_raw::<()>(hooks, |state: *mut lua_State| {
            let mut seen = Vec::new();
            freeze_value(state, &mut seen);
            ffi::lua_settop(state, 0);
        })
    }
}

/// Freezes what's on top of the stack and everything it reaches, leaving the stack as it was.
unsafe fn freeze_value(state: *mut lua_State, seen: &mut Vec<*const std::ffi::c_void>) {
    // SAFETY: the caller passes a live state with a value on top, and every push is checked for room.
    unsafe {
        let kind = ffi::lua_type(state, -1);
        if kind != ffi::LUA_TTABLE && kind != ffi::LUA_TFUNCTION {
            return;
        }
        let pointer = ffi::lua_topointer(state, -1);
        if seen.contains(&pointer) || ffi::lua_checkstack(state, 4) == 0 {
            return;
        }
        seen.push(pointer);
        if kind == ffi::LUA_TTABLE {
            ffi::lua_setreadonly(state, -1, 1);
            ffi::lua_pushnil(state);
            while ffi::lua_next(state, -2) != 0 {
                freeze_value(state, seen);
                ffi::lua_pop(state, 1);
                freeze_value(state, seen);
            }
        } else {
            let mut upvalue = 1;
            while !ffi::lua_getupvalue(state, -1, upvalue).is_null() {
                freeze_value(state, seen);
                ffi::lua_pop(state, 1);
                upvalue += 1;
            }
        }
    }
}

/// Removes every key of `table` not on the list.
fn keep_only(table: &Table, keep: &[&str]) -> mlua::Result<()> {
    let mut remove = Vec::new();
    table.for_each(|key: Value, _: Value| {
        let kept =
            matches!(&key, Value::String(name) if name.to_str().is_ok_and(|name| keep.contains(&&*name)));
        if !kept {
            remove.push(key);
        }
        Ok(())
    })?;
    for key in remove {
        table.raw_set(key, Value::Nil)?;
    }
    Ok(())
}
