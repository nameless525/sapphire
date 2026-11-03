//! The whole VM on whatever target this was built for, with no window: what a Simulator and an emulator
//! run to prove Luau links and works there, and what a binary's size is read from.
//!
//! ```text
//! cargo build --release -p sapphire_script --example smoke --target <target>
//! ```
//!
//! Prints a line per check and `smoke ok`, and fails otherwise. A C++ exception has to work for the
//! error rows to pass, so a C++ runtime that's wrongly linked shows here.

#![allow(clippy::print_stdout, reason = "its output is what it's for")]

use std::process::ExitCode;

use mlua::Function;
use sapphire_script::bindings::Frame;
use sapphire_script::{Command, HookEnd, Limits, ScriptVm, cook};

const FOLD: &str = r#"
return { hook = function()
    local h = 0
    for i = 1, 1000 do
        h = (h * 31 + math.floor(math.sin(i) * 1000) + i % 7) % 4294967296
    end
    local t = {}
    for i, word in ipairs({'a', 'bb', 'ccc', 'dddd', 'eeeee'}) do t[word] = i end
    for k, v in pairs(t) do h = (h * 31 + v + #k) % 4294967296 end
    local list = {5, 3, 9, 1, 7}
    table.sort(list)
    h = h + list[1] * 10 + list[5]
    sapphire.mark(h)
    sapphire.set_board(0, h)
end }
"#;

fn fire(vm: &ScriptVm, source: &str) -> (HookEnd, Frame) {
    let hooks = vm.load(&cook(source).expect("cooks"), "=smoke").expect("loads");
    let hook: Function = hooks.get("hook").expect("has a hook");
    let mut frame = Frame::new();
    let end = vm.fire(&mut frame, &hook, ());
    (end, frame)
}

fn main() -> ExitCode {
    let vm = ScriptVm::new(Limits { memory: 1 << 20, interrupts: 100_000 }).expect("the VM starts");
    let mut ok = true;
    let mut check = |name: &str, passed: bool, detail: String| {
        println!("{} {name}: {detail}", if passed { "pass" } else { "FAIL" });
        ok &= passed;
    };

    let (end, frame) = fire(&vm, FOLD);
    let value = match frame.queue[0] {
        Command::Mark(value) => value,
        _ => f64::NAN,
    };
    check(
        "a hook runs and pinned math reads",
        end == HookEnd::Returned,
        format!("{end:?} {:016x}", value.to_bits()),
    );

    let (end, _) = fire(&vm, "return { hook = function() error('a mistake') end }");
    check("error() is caught: C++ exceptions work", matches!(end, HookEnd::Failed(_)), format!("{end:?}"));

    let (end, _) = fire(&vm, "return { hook = function() local t = nil; return t.x end }");
    check("a type error is caught", matches!(end, HookEnd::Failed(_)), format!("{end:?}"));

    let (end, _) = fire(&vm, "return { hook = function() while true do end end }");
    check("a budget ends a loop by a yield", end == HookEnd::OverBudget, format!("{end:?}"));

    let (end, _) = fire(&vm, "return { hook = function() local t = {} for i = 1, 1e7 do t[i] = {} end end }");
    check("a memory limit or a budget ends a hoard", end != HookEnd::Returned, format!("{end:?}"));
    vm.lua().gc_collect().expect("collects");
    vm.lua().gc_stop();

    let (end, frame) = fire(&vm, "return { hook = function() sapphire.mark({}) end }");
    check(
        "a wrong argument faults, and raises nothing",
        end == HookEnd::Returned && frame.fault,
        format!("{end:?}"),
    );

    let finished = vm.gc_step().expect("collects");
    check(
        "a collection step leaves it stopped",
        !vm.lua().gc_is_running(),
        format!("cycle ended: {finished}"),
    );

    if ok {
        println!("smoke ok");
        ExitCode::SUCCESS
    } else {
        println!("smoke FAILED");
        ExitCode::FAILURE
    }
}
