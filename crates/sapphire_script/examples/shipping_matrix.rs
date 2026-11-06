//! What each way a script can go wrong does to the process, under whichever profile this was built in.
//! The point is `--profile shipping`, whose `panic = "abort"` a C++ exception can't cross a Rust frame
//! under.
//!
//! ```text
//! cargo run --profile shipping -p sapphire_script --example shipping_matrix
//! ```
//!
//! With no argument it runs itself once per case, as a subprocess, and prints each one's exit status: a
//! case that kills the process only kills its own subprocess. A case that hangs is killed at 5 s.
//! The parent fails if any process outcome differs from its declared expectation.

#![allow(clippy::print_stdout, reason = "a report's output is what it's for")]

use std::process::{Command as Process, ExitCode, Stdio};
use std::thread::sleep;
use std::time::Duration;

use mlua::thread::ThreadStatus;
use mlua::{Lua, VmState, ffi, lua_State};
use sapphire_script::bindings::Frame;
use sapphire_script::{HookArgs, HookEnd, Limits, ScriptVm, VmError, cook};

/// Exits 0 only by reaching the line after the failure: the process lived.
fn survived(what: impl std::fmt::Debug) -> ExitCode {
    println!("survived: {what:.90?}");
    ExitCode::SUCCESS
}

fn plain_lua() -> Lua {
    Lua::new()
}

/// A hook on the sandboxed VM, the shape the crate ships.
fn on_vm(source: &str, limits: Limits) -> HookEnd {
    on_vm_with_args(source, limits, ())
}

fn on_vm_with_args(source: &str, limits: Limits, args: impl Into<HookArgs>) -> HookEnd {
    let vm = ScriptVm::new(limits).expect("the VM starts");
    let cooked = cook(source).expect("cooks");
    let hooks = vm.load(&cooked, "=case").expect("loads");
    let hook = hooks.hook("hook").expect("hook type").expect("has a hook");
    let mut frame = Frame::new();
    vm.fire(&mut frame, &hook, args)
}

/// The raw callback scouts intentionally exercise unsupported foreign-unwind paths.
#[derive(Clone, Copy, Debug)]
enum Expected {
    Survives,
    Dies,
    TimesOut,
    DiesWhenAbort,
}

/// A named way to end, its process expectation, and its isolated probe.
type Case = (&'static str, Expected, fn() -> ExitCode);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HookOutcome {
    Returned,
    OverBudget,
    Failed,
}

fn expect_hook(end: HookEnd, expected: HookOutcome) -> ExitCode {
    let actual = match &end {
        HookEnd::Returned => HookOutcome::Returned,
        HookEnd::OverBudget => HookOutcome::OverBudget,
        HookEnd::Failed(_) => HookOutcome::Failed,
    };
    assert_eq!(actual, expected, "{end:?}");
    survived(end)
}

const CASES: &[Case] = &[
    // Through `mlua`'s own paths: the scout's reading.
    ("mlua binding returns Err, script catches nothing", Expected::DiesWhenAbort, || {
        let lua = plain_lua();
        let f = lua.create_function(|_, ()| -> mlua::Result<()> { Err(mlua::Error::runtime("no")) }).unwrap();
        lua.globals().set("binding", f).unwrap();
        survived(lua.load("binding()").exec())
    }),
    ("mlua binding returns Err, script's pcall catches", Expected::DiesWhenAbort, || {
        let lua = plain_lua();
        let f = lua.create_function(|_, ()| -> mlua::Result<()> { Err(mlua::Error::runtime("no")) }).unwrap();
        lua.globals().set("binding", f).unwrap();
        survived(lua.load("return pcall(binding)").eval::<(bool, String)>())
    }),
    ("mlua binding handed a wrong argument", Expected::DiesWhenAbort, || {
        let lua = plain_lua();
        let f = lua.create_function(|_, x: f64| Ok(x)).unwrap();
        lua.globals().set("binding", f).unwrap();
        survived(lua.load("binding({})").exec())
    }),
    ("mlua binding panics", Expected::Dies, || {
        let lua = plain_lua();
        let f = lua.create_function(|_, ()| -> mlua::Result<()> { panic!("a bug") }).unwrap();
        lua.globals().set("binding", f).unwrap();
        survived(lua.load("binding()").exec())
    }),
    ("mlua binding makes a string at the memory limit", Expected::DiesWhenAbort, || {
        let lua = plain_lua();
        let f = lua
            .create_function(|lua, ()| {
                let text = lua.create_string(vec![0u8; 1 << 20])?;
                Ok(text)
            })
            .unwrap();
        lua.globals().set("binding", f).unwrap();
        lua.set_memory_limit(lua.used_memory() + (64 << 10)).unwrap();
        survived(lua.load("binding()").exec())
    }),
    ("set_interrupt raises an error", Expected::DiesWhenAbort, || {
        let lua = plain_lua();
        lua.set_interrupt(|_| Err(mlua::Error::runtime("out of budget")));
        survived(lua.load("while true do end").exec())
    }),
    ("script error(), uncaught, on the main thread", Expected::Survives, || {
        let lua = plain_lua();
        survived(lua.load("error('a mistake')").exec())
    }),
    ("script error(), caught by its pcall", Expected::Survives, || {
        let lua = plain_lua();
        survived(lua.load("return pcall(error, 'a mistake')").eval::<(bool, String)>())
    }),
    ("script error(), uncaught, in a thread", Expected::Survives, || {
        let lua = plain_lua();
        let f = lua.load("error('a mistake')").into_function().unwrap();
        let thread = lua.create_thread(f).unwrap();
        survived((thread.resume::<()>(()), thread.status() == ThreadStatus::Error))
    }),
    ("a runtime type error in script", Expected::Survives, || {
        let lua = plain_lua();
        survived(lua.load("local t = nil; return t.x").exec())
    }),
    ("stack overflow by recursion", Expected::Survives, || {
        let lua = plain_lua();
        survived(lua.load("local function f(n) return 1 + f(n + 1) end return f(1)").exec())
    }),
    ("memory limit hit in script, main thread", Expected::Survives, || {
        let lua = plain_lua();
        lua.set_memory_limit(lua.used_memory() + (256 << 10)).unwrap();
        survived(lua.load("local t = {} for i = 1, 1e7 do t[i] = {} end").exec())
    }),
    ("memory limit hit in script, in a thread", Expected::Survives, || {
        let lua = plain_lua();
        lua.set_memory_limit(lua.used_memory() + (256 << 10)).unwrap();
        let f = lua.load("local t = {} for i = 1, 1e7 do t[i] = {} end").into_function().unwrap();
        let thread = lua.create_thread(f).unwrap();
        survived(thread.resume::<()>(()))
    }),
    ("VmState::Yield in a thread", Expected::Survives, || {
        let lua = plain_lua();
        lua.set_interrupt(|_| Ok(VmState::Yield));
        let f = lua.load("while true do end").into_function().unwrap();
        let thread = lua.create_thread(f).unwrap();
        let first = thread.resume::<()>(());
        let status = thread.status();
        let nothing = lua.create_function(|_, ()| Ok(())).unwrap();
        survived((first, status, thread.reset(nothing)))
    }),
    ("VmState::Yield on the main thread: can't yield, so never ends", Expected::TimesOut, || {
        let lua = plain_lua();
        lua.set_interrupt(|_| Ok(VmState::Yield));
        survived(lua.load("while true do end").exec())
    }),
    ("VmState::Yield before the native sort yields the coroutine", Expected::Survives, || {
        let lua = plain_lua();
        lua.set_interrupt(|_| Ok(VmState::Yield));
        let f = lua
            .load("local t = {3, 2, 1} table.sort(t, function(a, b) while true do end return a < b end)")
            .into_function()
            .unwrap();
        let thread = lua.create_thread(f).unwrap();
        survived(thread.resume::<()>(()))
    }),
    ("coroutine.yield in a thread, then resumed", Expected::Survives, || {
        let lua = plain_lua();
        let f = lua.load("coroutine.yield(1) return 2").into_function().unwrap();
        let thread = lua.create_thread(f).unwrap();
        survived((thread.resume::<i32>(()), thread.resume::<i32>(()), thread.status()))
    }),
    ("coroutine.yield across a pcall", Expected::Survives, || {
        let lua = plain_lua();
        let f =
            lua.load("return pcall(function() coroutine.yield(1) error('x') end)").into_function().unwrap();
        let thread = lua.create_thread(f).unwrap();
        survived((thread.resume::<i32>(()), thread.resume::<(bool, String)>(())))
    }),
    ("a raw C binding calls lua_error: what mlua's Err path comes to", Expected::DiesWhenAbort, || {
        unsafe extern "C-unwind" fn raising(state: *mut lua_State) -> i32 {
            // SAFETY: a live state, with its error on the stack. This is the case being measured.
            unsafe {
                ffi::lua_pushstring(state, c"no".as_ptr());
                ffi::lua_error(state)
            }
        }
        let lua = plain_lua();
        // SAFETY: the function is valid for any state: raising is what's measured.
        let f = unsafe { lua.create_c_function(raising) }.unwrap();
        lua.globals().set("binding", f).unwrap();
        survived(lua.load("binding()").exec())
    }),
    // The crate's shape: the same mistakes on `ScriptVm`, through `fire`.
    ("VM: a binding handed a wrong argument", Expected::Survives, || {
        expect_hook(
            on_vm("return { hook = function() sapphire.mark({}) end }", Limits::default()),
            HookOutcome::Returned,
        )
    }),
    ("VM: a budget ends an endless loop", Expected::Survives, || {
        expect_hook(
            on_vm("return { hook = function() while true do end end }", Limits::default()),
            HookOutcome::OverBudget,
        )
    }),
    ("VM: a budget ends an endless loop in a sort's comparison", Expected::Survives, || {
        expect_hook(
            on_vm(
                "return { hook = function() table.sort({3, 2, 1}, function(a, b) while true do end end) end }",
                Limits::default(),
            ),
            HookOutcome::OverBudget,
        )
    }),
    ("VM: a budget ends endless recursion through a binding's result", Expected::Survives, || {
        expect_hook(
            on_vm(
                "local function f(n) return f(n + sapphire.tick()) + 1 end return { hook = function() return f(1) end }",
                Limits::default(),
            ),
            HookOutcome::OverBudget,
        )
    }),
    ("VM: script error()", Expected::Survives, || {
        expect_hook(
            on_vm("return { hook = function() error('a mistake') end }", Limits::default()),
            HookOutcome::Failed,
        )
    }),
    ("VM: a checked prelude function refuses", Expected::Survives, || {
        expect_hook(
            on_vm("return { hook = function() return tostring({}) end }", Limits::default()),
            HookOutcome::Failed,
        )
    }),
    ("VM: a type error", Expected::Survives, || {
        expect_hook(
            on_vm("return { hook = function() local t = nil; return t.x end }", Limits::default()),
            HookOutcome::Failed,
        )
    }),
    ("VM: a global write", Expected::Survives, || {
        expect_hook(
            on_vm("return { hook = function() rawset(table, 'x', 1) end }", Limits::default()),
            HookOutcome::Failed,
        )
    }),
    ("VM: the memory limit in a table", Expected::Survives, || {
        expect_hook(
            on_vm(
                "return { hook = function() local t = {} for i = 1, 1e7 do t[i] = {} end end }",
                Limits { memory: 1 << 20, interrupts: u64::MAX, ..Limits::default() },
            ),
            HookOutcome::Failed,
        )
    }),
    ("VM: the memory limit in table.create", Expected::Survives, || {
        expect_hook(
            on_vm(
                "return { hook = function() return table.create(1e7, 0) end }",
                Limits { memory: 1 << 20, interrupts: u64::MAX, ..Limits::default() },
            ),
            HookOutcome::Failed,
        )
    }),
    ("VM: hooks, a load and a collection with the memory limit full", Expected::Survives, || {
        let vm = ScriptVm::new(Limits { memory: 1 << 20, interrupts: u64::MAX, ..Limits::default() })
            .expect("the VM starts");
        let hoard =
            cook("return { hook = function() local t = {} for i = 1, 1e7 do t[i] = {} end end }").unwrap();
        let hook = vm.load(&hoard, "=case").unwrap().hook("hook").unwrap().unwrap();
        let mut frame = Frame::new();
        // Full, and with the collector stopped it stays full for what comes next.
        let first = vm.fire(&mut frame, &hook, ());
        let deep =
            cook("local function f(n) return 1 + f(n + 1) end return { hook = function() return f(1) end }");
        let loaded = vm.load(&deep.unwrap(), "=case").map(|_| ());
        let mut ends = Vec::new();
        for _ in 0..8 {
            ends.push(vm.fire(&mut frame, &hook, (1.0, 2.0, 3.0)));
        }
        let step = vm.gc_step().map(|_| ());
        while !vm.gc_step().expect("collects") {}
        let after = vm.fire(&mut frame, &hook, ());
        assert!(matches!(first, HookEnd::Failed(_)));
        assert!(ends.iter().all(|end| matches!(end, HookEnd::Failed(_))));
        assert!(step.is_ok());
        assert!(matches!(after, HookEnd::Failed(_)));
        survived((first, loaded.is_ok(), ends.len(), step.is_ok(), after))
    }),
    ("VM: initializer loops end without a foreign error", Expected::Survives, || {
        let vm = ScriptVm::new(Limits { interrupts: 20, ..Limits::default() }).unwrap();
        for source in
            ["while true do end return {}", "local function f() while true do end end f() return {}"]
        {
            let error = vm.load(&cook(source).unwrap(), "=initializer").unwrap_err();
            assert!(matches!(error, VmError::OverBudget));
        }
        survived(vm.load(&cook("return {hook = function() end}").unwrap(), "=finite").map(|_| ()))
    }),
    ("VM: invalid table positions raise only in Luau", Expected::Survives, || {
        for (body, position) in [
            ("table.insert({1}, position, 9)", 1.5),
            ("table.remove({1}, position)", f64::NAN),
            ("table.find({1}, 1, position)", f64::INFINITY),
        ] {
            expect_hook(
                on_vm_with_args(
                    &format!("return {{hook = function(position) {body} end}}"),
                    Limits::default(),
                    position,
                ),
                HookOutcome::Failed,
            );
        }
        ExitCode::SUCCESS
    }),
    ("VM: initializer memory errors stay inside Luau", Expected::Survives, || {
        let vm =
            ScriptVm::new(Limits { memory: 1 << 20, interrupts: u64::MAX, ..Limits::default() }).unwrap();
        let loaded = vm.load(&cook("return table.create(1e7, 0)").unwrap(), "=initializer");
        assert!(matches!(loaded, Err(VmError::Lua(mlua::Error::MemoryError(_)))));
        survived(loaded.map(|_| ()))
    }),
    ("VM: the memory limit growing the stack", Expected::Survives, || {
        expect_hook(
            on_vm(
                "local function f(n) return 1 + f(n + 1) end return { hook = function() return f(1) end }",
                Limits { memory: 256 << 10, interrupts: u64::MAX, ..Limits::default() },
            ),
            HookOutcome::Failed,
        )
    }),
];

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    if let Some(case) = args.next() {
        let index: usize = case.parse().expect("a case's number");
        return (CASES[index].2)();
    }
    let me = std::env::current_exe().expect("this program's path");
    println!("panic strategy: {}", if cfg!(panic = "abort") { "abort" } else { "unwind" });
    let mut failures = 0;
    for (index, (name, expected, _)) in CASES.iter().enumerate() {
        let mut child = Process::new(&me)
            .arg(index.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("runs");
        let mut waited = 0;
        let status = loop {
            if let Some(status) = child.try_wait().expect("waits") {
                break Some(status);
            }
            if waited >= 500 {
                child.kill().expect("kills");
                child.wait().expect("waits");
                break None;
            }
            waited += 1;
            sleep(Duration::from_millis(10));
        };
        let expected = match expected {
            Expected::DiesWhenAbort if cfg!(panic = "abort") => Expected::Dies,
            Expected::DiesWhenAbort => Expected::Survives,
            expected => *expected,
        };
        let matched = match (expected, status) {
            (Expected::TimesOut, None) => true,
            (Expected::Survives, Some(status)) => status.success(),
            (Expected::Dies, Some(status)) => !status.success(),
            _ => false,
        };
        if !matched {
            failures += 1;
        }
        let read = match status {
            None => "HANGS (killed at 5 s)".to_owned(),
            Some(status) if status.success() => "survives".to_owned(),
            Some(status) => format!("DIES: {status}"),
        };
        println!("{read:<28} {name} (expected {expected:?}, {})", if matched { "ok" } else { "MISMATCH" });
    }
    if failures == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}
