//! The VM: Luau with an allowlist of libraries, read-only, on a count rather than a clock.

use std::cell::Cell;
use std::ffi::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mlua::chunk::ChunkMode;
use mlua::state::{GcIncParams, GcMode};
use mlua::thread::ThreadStatus;
use mlua::{Function, Lua, LuaOptions, StdLib, Table, Thread, Value, ffi, lua_State};
use sapphire_bytecode::compile::{self, CookedScript};
use sapphire_bytecode::{CookError, check};
use sapphire_core::alloc::{HostAllocations, VmAllocations};
use sapphire_gameplay::HookFrame;

use crate::bindings::{self, Binding};

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
    /// Interrupts one step's hooks may spend, all told. Once they've gone, what's left of the step's events
    /// goes unanswered: a crowd's hits on one tick mustn't be `interrupts` times itself. At its default, one
    /// budget, the worst a step can spend is the worst one hook can.
    pub step_interrupts: u64,
    /// What one step's hooks are held to in [`crate::ScriptStats::over_budget_steps`]: spec §3.2's row, the
    /// desktop's by default, for an app to replace with its tier's.
    pub step_budget: Duration,
}

impl Default for Limits {
    /// Read from a Mac: at 10,000 interrupts a spinning hook ends in 25 µs, or 90 µs through bindings, and
    /// a real hook takes tens. 2 MiB holds the VM's own 520 KB and a boss's garbage between steps, and at
    /// 16 KB a step a 2 MiB heap's cycle takes about 50 steps.
    fn default() -> Self {
        Self {
            memory: 2 << 20,
            interrupts: 10_000,
            gc_step_kb: 16,
            step_interrupts: 10_000,
            step_budget: Duration::from_micros(300),
        }
    }
}

/// An opaque frozen hook table. Lookup accepts only a name, so caller-defined mlua conversion traits
/// cannot obtain the underlying Lua state through a supposedly ordinary lookup.
///
/// ```compile_fail
/// fn arbitrary(table: &sapphire_script::HookTable) {
///     let _: mlua::Value = table.get("hook").unwrap();
/// }
/// ```
///
/// ```compile_fail
/// struct Capture;
/// impl mlua::FromLua for Capture {
///     fn from_lua(_: mlua::Value, _: &mlua::Lua) -> mlua::Result<Self> { Ok(Self) }
/// }
/// fn arbitrary(table: &sapphire_script::HookTable) {
///     let _ = table.hook::<Capture>("hook");
/// }
/// ```
///
/// ```compile_fail
/// fn arbitrary(table: &sapphire_script::HookTable, key: impl mlua::IntoLua) {
///     let _ = table.hook(key);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct HookTable {
    table: Table,
}

impl HookTable {
    /// A named hook, `None` for a missing/nil field, or a type error for anything but a function.
    pub fn hook(&self, name: &str) -> Result<Option<Hook>, VmError> {
        let _vm = VmAllocations::enter();
        let function = self.table.raw_get::<Option<Function>>(name)?;
        Ok(function.map(|function| Hook { function }))
    }

    /// The chunk's `names`, the moves its commands and `on_move` name by place, or none if it lists none.
    pub fn names(&self) -> Result<Vec<String>, VmError> {
        let _vm = VmAllocations::enter();
        Ok(self.table.raw_get::<Option<Vec<String>>>("names")?.unwrap_or_default())
    }

    /// The chunk's `conditions`, a dialogue's, each a function of no arguments asked through
    /// [`ScriptVm::ask`], or none if it lists none.
    pub fn conditions(&self) -> Result<Vec<Hook>, VmError> {
        let _vm = VmAllocations::enter();
        let functions = self.table.raw_get::<Option<Vec<Function>>>("conditions")?.unwrap_or_default();
        Ok(functions.into_iter().map(|function| Hook { function }).collect())
    }

    /// Whether the table is frozen, including after a trusted diagnostic integration.
    #[must_use]
    pub fn is_readonly(&self) -> bool {
        self.table.is_readonly()
    }

    /// The raw table for diagnostics that deliberately bypass the facade.
    ///
    /// # Safety
    ///
    /// This handle and its generic conversions can expose the unrestricted Lua state. All binary
    /// code executed through it or retained handles must be unmodified, structurally valid output of
    /// the exact linked compiler. The caller must preserve sandbox, immutability and execution rules.
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn table_for_tests(&self) -> &Table {
        &self.table
    }
}

/// An opaque hook selected from a [`HookTable`], retaining its owning VM identity.
///
/// ```compile_fail
/// fn arbitrary(hook: &sapphire_script::Hook) {
///     hook.call::<()>(()).unwrap();
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Hook {
    function: Function,
}

impl Hook {
    /// The raw function for integrations that deliberately bypass numeric event arguments.
    ///
    /// # Safety
    ///
    /// This handle can expose the unrestricted Lua state through caller conversions. All binary
    /// code executed through it or retained handles must be unmodified, structurally valid output of
    /// the exact linked compiler. The caller must preserve sandbox, immutability and execution rules.
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn function_for_tests(&self) -> &Function {
        &self.function
    }
}

/// Numeric arguments for the engine's event hooks: `on_phase` has three, and `on_beat` has one.
/// The owned value keeps arbitrary Rust conversion outside VM allocation attribution while numeric
/// stack pushes and Lua execution remain inside it. Tables, strings and arbitrary mlua conversions
/// are deliberately excluded from this fixed-step event boundary.
///
/// ```compile_fail
/// use sapphire_gameplay::HookFrame;
/// use sapphire_script::ScriptVm;
/// fn arbitrary(vm: &ScriptVm, frame: &mut HookFrame, hook: &sapphire_script::Hook, args: mlua::MultiValue) {
///     vm.fire(frame, hook, args);
/// }
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum HookArgs {
    #[default]
    None,
    One(f64),
    Two(f64, f64),
    Three(f64, f64, f64),
}

impl From<()> for HookArgs {
    fn from((): ()) -> Self {
        Self::None
    }
}

impl From<f64> for HookArgs {
    fn from(value: f64) -> Self {
        Self::One(value)
    }
}

impl From<(f64,)> for HookArgs {
    fn from((value,): (f64,)) -> Self {
        Self::One(value)
    }
}

impl From<(f64, f64)> for HookArgs {
    fn from((first, second): (f64, f64)) -> Self {
        Self::Two(first, second)
    }
}

impl From<(f64, f64, f64)> for HookArgs {
    fn from((first, second, third): (f64, f64, f64)) -> Self {
        Self::Three(first, second, third)
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
    #[error("the script initializer exceeded its interrupt budget")]
    OverBudget,
    #[error("the script's reachable graph exceeds the VM stack or memory limit")]
    FreezeStack,
    #[error("the binding `{0}` didn't go through `with_frame`, so what it allocates wouldn't reach gate 4")]
    UnflippedBinding(&'static str),
    #[error("dialogue `{name}`'s chunk makes {found} conditions where its graph asks {expected}")]
    ConditionCount { name: String, expected: usize, found: usize },
}

thread_local! {
    /// Interrupts the running hook has left. A count kept here because the interrupt sees only a state.
    static BUDGET: Cell<u64> = const { Cell::new(0) };
    /// Interrupts since the VM started on this thread: how a budget's read.
    static INTERRUPTS: Cell<u64> = const { Cell::new(0) };
}

/// Saves a caller's remaining budget when a host callback enters another VM.
struct BudgetGuard(u64);

impl BudgetGuard {
    fn enter(interrupts: u64) -> Self {
        Self(BUDGET.replace(interrupts))
    }
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        BUDGET.set(self.0);
    }
}

/// Reentry is refused for the whole reset/argument conversion/resume transaction. The VM remains
/// `Sync` as an ECS resource; unlike a mutex, this guard neither blocks callbacks nor poisons on panic.
struct HookGuard<'a>(&'a AtomicBool);

impl Drop for HookGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
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
    firing: AtomicBool,
    limits: Limits,
}

impl ScriptVm {
    /// A sandboxed VM with nothing loaded, the one way to make one: the allowlist applied, the prelude
    /// run, the globals frozen, the budget's interrupt set, the memory limit on and the collector
    /// stopped, so it runs only when [`Self::gc_step`] says.
    pub fn new(limits: Limits) -> Result<Self, VmError> {
        Self::with_bindings(limits, &[])
    }

    /// [`Self::new`], with more bindings put in `sapphire` beside the declared table's before the sandbox
    /// closes: how a test adds one that misbehaves. Each is called once here, with no frame and no
    /// arguments, and one that doesn't go through [`bindings::with_frame`] is refused.
    pub fn with_bindings(limits: Limits, more: &[Binding]) -> Result<Self, VmError> {
        let lua = Lua::new_with(
            StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::BIT | StdLib::VECTOR,
            LuaOptions::new(),
        )?;
        let _frame = bindings::without_frame();
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
        Ok(Self { lua, thread, firing: AtomicBool::new(false), limits })
    }

    /// The unrestricted Luau state for diagnostics that deliberately bypass this facade's policy,
    /// execution budgets, collector scheduling and allocation attribution.
    ///
    /// # Safety
    ///
    /// Any binary bytecode executed through this state must be complete, structurally valid output
    /// of the exact linked compiler, with all of Luau's instruction/reference invariants preserved.
    /// The facade's safe [`Self::load`] cannot establish provenance for operations through this handle.
    #[doc(hidden)]
    #[must_use]
    pub const unsafe fn lua_for_tests(&self) -> &Lua {
        &self.lua
    }

    /// Whether the collector is stopped, as it should be whenever a step isn't asking it for work.
    #[must_use]
    pub fn collector_stopped(&self) -> bool {
        !self.lua.gc_is_running()
    }

    /// Runs bytecode with compiler provenance, rechecks its deterministic instruction policy, and
    /// returns the table of hooks it makes. Allocates, so it happens between steps. Raw serialized
    /// bytes require the explicit trust contract on [`CookedScript::from_trusted_bytes`].
    ///
    /// ```compile_fail
    /// use sapphire_script::{Limits, ScriptVm};
    /// let vm = ScriptVm::new(Limits::default()).unwrap();
    /// vm.load(&[9, 1, 0, 0, 0], "=untrusted");
    /// ```
    pub fn load(&self, cooked: &CookedScript, name: &str) -> Result<HookTable, VmError> {
        check(cooked.as_ref())?;
        let _vm = VmAllocations::enter();
        let _frame = bindings::without_frame();
        let _budget = BudgetGuard::enter(self.limits.interrupts);
        // The main Lua thread cannot yield from an interrupt. A separate coroutine also leaves a
        // hook's reusable coroutine untouched when a host callback loads another chunk.
        let chunk =
            self.lua.load(cooked.as_ref()).set_name(name).set_mode(ChunkMode::Binary).into_function()?;
        let thread = self.lua.create_thread(chunk.clone())?;
        let resumed = thread.resume::<Value>(());
        let value = match resumed {
            Ok(value) => value,
            Err(error) => {
                let _ = thread.reset(chunk);
                return Err(error.into());
            }
        };
        if !matches!(thread.status(), ThreadStatus::Finished) {
            // A suspended initializer keeps its stack alive; reset it before releasing the handle.
            thread.reset(chunk)?;
            return Err(VmError::OverBudget);
        }
        let Value::Table(hooks) = value else { return Err(VmError::NotAHookTable) };
        freeze_reachable(&self.lua, &hooks)?;
        Ok(HookTable { table: hooks })
    }

    /// Runs one hook to its end, with `frame` for its bindings to read and write. A callback may enter
    /// another VM, but reentering this VM's reusable hook coroutine fails before it is changed.
    /// Caller-provided conversion into [`HookArgs`] runs as host work, so its Rust allocations reach
    /// the fixed-step gate. Only the resulting numeric stack pushes and VM work use VM attribution.
    pub fn fire(&self, frame: &mut HookFrame, hook: &Hook, args: impl Into<HookArgs>) -> HookEnd {
        // Also covers caller-owned argument destruction when ownership/reentry rejects preparation.
        let _host = HostAllocations::enter();
        self.fire_prepared(
            frame,
            &hook.function,
            || args.into(),
            |thread, args| {
                // Built-in numeric conversions push directly onto the Lua stack without a Rust
                // MultiValue allocation; any stack growth is correctly attributed to the VM.
                match args {
                    HookArgs::None => thread.resume::<()>(()),
                    HookArgs::One(value) => thread.resume::<()>(value),
                    HookArgs::Two(first, second) => thread.resume::<()>((first, second)),
                    HookArgs::Three(first, second, third) => thread.resume::<()>((first, second, third)),
                }
            },
        )
    }

    /// Runs an integration hook with unrestricted mlua argument conversion. Unlike [`Self::fire`],
    /// conversion runs in the VM allocation scope; custom Rust work must use [`bindings::in_host`]
    /// to reach the fixed-step allocation gate. This retains arbitrary argument capabilities for
    /// trusted integrations, outside the numeric event path's automatic attribution guarantee.
    ///
    /// # Safety
    ///
    /// Argument conversion receives this VM's unrestricted Lua state. Any binary code executed
    /// through it or handles retained from it must be complete, structurally valid, unmodified output
    /// of the exact linked compiler. The caller must preserve the facade's sandbox/immutability and
    /// execution/accounting invariants; the normal load policy
    /// cannot constrain this callback.
    pub unsafe fn fire_trusted(
        &self,
        frame: &mut HookFrame,
        hook: &Function,
        args: impl mlua::IntoLuaMulti,
    ) -> HookEnd {
        self.fire_prepared(frame, hook, || args, |thread, args| thread.resume::<()>(args))
    }

    /// One lifecycle for numeric events and explicit trusted integrations. Preparation is host work;
    /// only reset/resume uses VM attribution. The guards cover caller conversion as well as execution.
    fn fire_prepared<A>(
        &self,
        frame: &mut HookFrame,
        hook: &Function,
        prepare: impl FnOnce() -> A,
        resume: impl FnOnce(&Thread, A) -> mlua::Result<()>,
    ) -> HookEnd {
        if hook.weak_lua() != &self.lua.weak() {
            return HookEnd::Failed("the hook belongs to another VM".to_owned());
        }
        if self.firing.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            return HookEnd::Failed("the VM is already running a hook".to_owned());
        }
        let _running = HookGuard(&self.firing);
        let thread = &self.thread;
        let _frame = bindings::enter(frame);
        let _budget = BudgetGuard::enter(self.limits.interrupts);
        let args = bindings::in_host(prepare);
        let _vm = VmAllocations::enter();
        if let Err(error) = thread.reset(hook.clone()) {
            return HookEnd::Failed(error.to_string());
        }
        match resume(thread, args) {
            Err(error) => HookEnd::Failed(error.to_string()),
            Ok(()) if matches!(thread.status(), ThreadStatus::Finished) => HookEnd::Returned,
            Ok(()) => {
                // A yielded thread holds its stack, and all a hook's garbage with it, until the next hook.
                let _ = thread.reset(hook.clone());
                HookEnd::OverBudget
            }
        }
    }

    /// Asks a dialogue's condition, with `frame` (read-only, between steps) for its bindings to read: it
    /// holds only if it returns `true`. Anything else it returns, an error or running out of budget
    /// doesn't hold.
    pub fn ask(&self, frame: &mut HookFrame, condition: &Hook) -> (HookEnd, bool) {
        let _vm = VmAllocations::enter();
        let _frame = bindings::enter(frame);
        BUDGET.set(self.limits.interrupts);
        if let Err(error) = self.thread.reset(condition.function.clone()) {
            return (HookEnd::Failed(error.to_string()), false);
        }
        match self.thread.resume::<Value>(()) {
            Err(error) => (HookEnd::Failed(error.to_string()), false),
            Ok(value) if matches!(self.thread.status(), ThreadStatus::Finished) => {
                (HookEnd::Returned, matches!(value, Value::Boolean(true)))
            }
            Ok(_) => {
                let _ = self.thread.reset(condition.function.clone());
                (HookEnd::OverBudget, false)
            }
        }
    }

    /// The limits it runs under.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
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
fn freeze_reachable(lua: &Lua, hooks: &Table) -> Result<(), VmError> {
    let complete = Cell::new(false);
    // SAFETY: `exec_raw` hands over a live state with `hooks` on top. The walk checks stack capacity
    // before pushing, uses only values/keys returned by Lua, and reports failure without raising.
    unsafe {
        lua.exec_raw::<()>(hooks, |state: *mut lua_State| {
            complete.set(freeze_graph(state));
            ffi::lua_settop(state, 0);
        })?;
    }
    if complete.get() { Ok(()) } else { Err(VmError::FreezeStack) }
}

/// Where to continue after the current child has been visited. Objects and iterator keys stay rooted
/// on the Lua stack, so no registry allocations are needed and cycles cannot invalidate a pointer.
#[derive(Clone, Copy)]
enum FreezeWalk {
    Visit,
    TableNext,
    TableKey,
    PopValue,
    Upvalue(c_int),
}

/// Visits the value on top, leaving it there. The explicit walk avoids native recursion, and its
/// visited set bounds membership work by reachable objects/edges rather than their square.
#[allow(
    clippy::disallowed_types,
    reason = "load-time membership only; the set is never iterated and cannot change traversal order"
)]
unsafe fn freeze_graph(state: *mut lua_State) -> bool {
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    let mut walk = vec![FreezeWalk::Visit];
    // SAFETY: the caller passes a live state with a value on top. Each new object reserves room for
    // its iterator/upvalue before pushing, and a failed reservation aborts the whole load.
    unsafe {
        while let Some(next) = walk.pop() {
            match next {
                FreezeWalk::Visit => {
                    let kind = ffi::lua_type(state, -1);
                    if kind != ffi::LUA_TTABLE && kind != ffi::LUA_TFUNCTION {
                        continue;
                    }
                    if !seen.insert(ffi::lua_topointer(state, -1)) {
                        continue;
                    }
                    if ffi::lua_checkstack(state, 4) == 0 {
                        return false;
                    }
                    if kind == ffi::LUA_TTABLE {
                        ffi::lua_setreadonly(state, -1, 1);
                        ffi::lua_pushnil(state);
                        walk.push(FreezeWalk::TableNext);
                    } else {
                        walk.push(FreezeWalk::Upvalue(1));
                    }
                }
                FreezeWalk::TableNext => {
                    if ffi::lua_next(state, -2) != 0 {
                        walk.push(FreezeWalk::TableKey);
                        walk.push(FreezeWalk::Visit);
                    }
                }
                FreezeWalk::TableKey => {
                    ffi::lua_pop(state, 1);
                    walk.push(FreezeWalk::TableNext);
                    walk.push(FreezeWalk::Visit);
                }
                FreezeWalk::PopValue => ffi::lua_pop(state, 1),
                FreezeWalk::Upvalue(index) => {
                    if !ffi::lua_getupvalue(state, -1, index).is_null() {
                        walk.push(FreezeWalk::Upvalue(index + 1));
                        walk.push(FreezeWalk::PopValue);
                        walk.push(FreezeWalk::Visit);
                    }
                }
            }
        }
    }
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_graph_that_cannot_reserve_stack_space_is_refused() {
        let lua = Lua::new();
        lua.gc_stop();
        let root: Table = lua.load("local root = {} local t = root for i = 1, 1000 do local child = {} t.child = child t = child end return root").eval().unwrap();
        lua.set_memory_limit(lua.used_memory()).unwrap();
        assert!(matches!(freeze_reachable(&lua, &root), Err(VmError::FreezeStack)));
        lua.set_memory_limit(0).unwrap();
        freeze_reachable(&lua, &root).unwrap();
        let mut child = root;
        for _ in 0..1000 {
            assert!(child.is_readonly());
            child = child.get("child").unwrap();
        }
        assert!(child.is_readonly());
    }

    #[test]
    fn budget_guards_restore_the_caller_after_return_and_unwind() {
        let _outer = BudgetGuard::enter(12);
        {
            let _inner = BudgetGuard::enter(3);
            BUDGET.set(1);
        }
        assert_eq!(BUDGET.get(), 12);
        let caught = std::panic::catch_unwind(|| {
            let _inner = BudgetGuard::enter(9);
            panic!("host callback");
        });
        assert!(caught.is_err());
        assert_eq!(BUDGET.get(), 12);
    }
}
