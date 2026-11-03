//! What a hook and a collection step cost, against spec §3.2's row (0.3 ms desktop, 0.5 ms mobile a
//! frame, hooks and the collector together).
//!
//! ```text
//! cargo run --release -p sapphire_script --example hook_cost
//! ```
//!
//! Each row fires a warm hook many times and reads the quietest tenth, the median and the slowest: the
//! slowest is mostly the scheduler while the machine's busy.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::alloc::System;
use std::time::Duration;

use mlua::Function;
use mlua::state::{GcIncParams, GcMode};
use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_script::bindings::Frame;
use sapphire_script::compile::{self, cook};
use sapphire_script::{HookEnd, Limits, ScriptVm};
use sapphire_time::Stopwatch;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

/// A boss's phase change: reads, a little arithmetic, a table of tuning, a loop, commands.
const BOSS_PHASE: &str = r#"
local TUNING = {
    {slow = 500, steps = 12, rage = 1.0, adds = 2},
    {slow = 250, steps = 20, rage = 1.5, adds = 3},
    {slow = 200, steps = 30, rage = 2.0, adds = 5},
}
return { hook = function()
    local phase = sapphire.tick() % 3 + 1
    local health = sapphire.health(1) + 100
    local tuning = TUNING[phase]
    local rage = tuning.rage * (1 + (1 - health / (health + 100)) * 0.5)
    sapphire.set_board(4, phase)
    sapphire.set_board(5, rage)
    sapphire.slow_motion(tuning.slow, tuning.steps)
    for add = 1, tuning.adds do
        sapphire.mark(add * rage)
    end
end }
"#;

/// The same decision made carelessly: fresh tables, strings and closures on every call.
const CARELESS: &str = r#"
local function plan(phase)
    return { name = "phase_" .. phase, adds = {}, slow = phase == 1 and 500 or 250 }
end
return { hook = function()
    local phase = sapphire.tick() % 3 + 1
    local p = plan(phase)
    for i = 1, 12 do
        p.adds[i] = {index = i, rage = 100 / i}
    end
    table.sort(p.adds, function(a, b) return a.rage < b.rage end)
    sapphire.set_board(4, #p.adds)
    sapphire.slow_motion(p.slow, 10)
    sapphire.mark(#p.name)
end }
"#;

/// A hook that does nothing: what it costs to get in and out.
const EMPTY: &str = "return { hook = function() end }";

const SPIN: &str = "return { hook = function() while true do end end }";
const SPIN_CALLS: &str =
    "local function f() return 1 end return { hook = function() while true do f() end end }";
const SPIN_MATH: &str =
    "return { hook = function() local x = 1 while true do x = math.sin(x) + sapphire.health(0) end end }";
const SPIN_STRINGS: &str =
    "return { hook = function() while true do local s = string.rep('x', 4096) end end }";
const SPIN_TABLES: &str = "return { hook = function() while true do local t = {1, 2, 3, 4} end end }";

fn microseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn load(vm: &ScriptVm, bytes: &[u8]) -> Function {
    vm.load(bytes, "=bench").expect("loads").get("hook").expect("has a hook")
}

/// The quietest tenth, the median and the slowest of `times`, in µs.
fn read(times: &mut [Duration]) -> (f64, f64, f64) {
    times.sort_unstable();
    (
        microseconds(times[times.len() / 10]),
        microseconds(times[times.len() / 2]),
        microseconds(times[times.len() - 1]),
    )
}

fn fire_row(name: &str, bytes: &[u8], collect: Collector) {
    const WARM: usize = 2_000;
    const FIRES: usize = 20_000;
    let vm = ScriptVm::new(Limits { memory: 1 << 30, ..Limits::default() }).expect("starts");
    let hook = load(&vm, bytes);
    let mut frame = Frame::new();
    if matches!(collect, Collector::Running) {
        vm.lua().gc_restart();
    }
    let mut times = Vec::with_capacity(FIRES);
    let mut gc_times = Vec::with_capacity(FIRES);
    let mut allocations = 0;
    for fire in 0..WARM + FIRES {
        frame.tick = fire as u64;
        frame.queued = 0;
        let before = alloc::vm_counts().allocations;
        let stopwatch = Stopwatch::start();
        let end = vm.fire(&mut frame, &hook, ());
        let elapsed = stopwatch.elapsed();
        let spent = alloc::vm_counts().allocations - before;
        assert_eq!(end, HookEnd::Returned);
        assert!(!frame.fault);
        if fire >= WARM {
            times.push(elapsed);
            allocations += spent;
        }
        if matches!(collect, Collector::StepEachFire) {
            let stopwatch = Stopwatch::start();
            vm.gc_step().expect("collects");
            if fire >= WARM {
                gc_times.push(stopwatch.elapsed());
            }
        }
    }
    let (quiet, median, slowest) = read(&mut times);
    println!(
        "{name:<34} quiet {quiet:7.2}  median {median:7.2}  slowest {slowest:8.1} µs   {:.2} VM allocations a fire",
        allocations as f64 / FIRES as f64,
    );
    if !gc_times.is_empty() {
        let (quiet, median, slowest) = read(&mut gc_times);
        println!(
            "{:<34} quiet {quiet:7.2}  median {median:7.2}  slowest {slowest:8.1} µs",
            "  and its gc_step"
        );
    }
}

#[derive(Clone, Copy)]
enum Collector {
    Stopped,
    StepEachFire,
    Running,
}

fn budget_row(name: &str, source: &str, limits: Limits) {
    let vm = ScriptVm::new(limits).expect("starts");
    let hook = load(&vm, &cook(source).expect("cooks"));
    let mut frame = Frame::new();
    let mut times = Vec::new();
    let mut end = HookEnd::Returned;
    for _ in 0..12 {
        let before = ScriptVm::interrupts();
        let stopwatch = Stopwatch::start();
        end = vm.fire(&mut frame, &hook, ());
        times.push((stopwatch.elapsed(), ScriptVm::interrupts() - before));
        vm.lua().gc_collect().expect("collects");
        vm.lua().gc_stop();
    }
    times.sort_unstable();
    let (quiet, count) = times[1];
    let end = match end {
        HookEnd::Failed(_) => "failed",
        HookEnd::OverBudget => "over budget",
        HookEnd::Returned => "returned",
    };
    println!(
        "{name:<34} {:9.1} µs to end ({end}), {count} interrupts, {:.1} ns an interrupt",
        microseconds(quiet),
        quiet.as_secs_f64() * 1e9 / count.max(1) as f64,
    );
}

/// A heap of about `bytes` live, then the collection steps of whole cycles over it, with garbage made in
/// between as a hook would.
/// What a heap is held in: a table is marked whole, so one big one is a step no step size can cut.
#[derive(Clone, Copy, Debug)]
enum Held {
    OneTable,
    SmallTables,
}

fn gc_row(bytes: usize, step_size_kb: Option<i32>, shape: Held) {
    const CYCLES: usize = 24;
    let vm = ScriptVm::new(Limits { memory: 256 << 20, interrupts: u64::MAX }).expect("starts");
    if let Some(size) = step_size_kb {
        vm.lua().gc_set_mode(GcMode::Incremental(GcIncParams::default().step_size(size)));
    }
    let held = match shape {
        Held::OneTable => "local keep = table.create(n) for i = 1, n do keep[i] = {i} end",
        Held::SmallTables => {
            "local keep = {} for c = 1, n // 256 do local t = table.create(256) for i = 1, 256 do t[i] = {i} end keep[c] = t end"
        }
    };
    let hook = load(
        &vm,
        &cook(&format!(
            "return {{ hook = function(n) {held} sapphire.set_board(0, #keep) return keep end }}"
        ))
        .expect("cooks"),
    );
    // Held live by a registry reference rather than a hook's stack.
    let keep: mlua::Value = hook.call((bytes / 64) as f64).unwrap_or(mlua::Value::Nil);
    let used = vm.used_memory();
    // A cycle's steps come in the same order each time, so each step's quickest over many cycles is its
    // own cost with the scheduler taken out, on a machine that isn't idle. The first cycle only warms.
    while !vm.gc_step().expect("collects") {}
    let mut quickest: Vec<Duration> = Vec::new();
    let mut steps = 0;
    for _ in 0..CYCLES {
        steps = 0;
        loop {
            let stopwatch = Stopwatch::start();
            let finished = vm.gc_step().expect("collects");
            let time = stopwatch.elapsed();
            if steps == quickest.len() {
                quickest.push(time);
            } else {
                quickest[steps] = quickest[steps].min(time);
            }
            steps += 1;
            if finished {
                break;
            }
        }
    }
    assert!(!vm.lua().gc_is_running(), "a step left the collector stopped");
    let cycle: Duration = quickest.iter().take(steps).sum();
    let mut sorted = quickest[..steps].to_vec();
    sorted.sort_unstable();
    println!(
        "{shape:?} heap {:>7} KB, now {:>7} KB, step size {step_size_kb:>5?}: {steps:>6} steps a cycle, median step {:6.2}, the slowest step {:8.2} µs; a cycle {:8.1} µs",
        used >> 10,
        vm.used_memory() >> 10,
        microseconds(sorted[steps / 2]),
        microseconds(sorted[steps - 1]),
        microseconds(cycle),
    );
    drop(keep);
}

fn main() {
    println!("hooks, one fire:");
    for (name, source) in [("empty", EMPTY), ("boss phase", BOSS_PHASE), ("careless", CARELESS)] {
        let cooked = cook(source).expect("cooks");
        fire_row(&format!("{name}, level 1, gc stopped"), &cooked, Collector::Stopped);
        fire_row(&format!("{name}, level 1, gc step each"), &cooked, Collector::StepEachFire);
        fire_row(&format!("{name}, level 1, gc running"), &cooked, Collector::Running);
        let level2 = compile::compile_unchecked(source, 2).expect("compiles");
        fire_row(&format!("{name}, level 2, gc step each"), &level2, Collector::StepEachFire);
    }
    let limits = Limits::default();
    println!(
        "what a budget bounds, at the default {} interrupts and {} MiB:",
        limits.interrupts,
        limits.memory >> 20
    );
    budget_row("empty loop", SPIN, limits);
    budget_row("loop of Luau calls", SPIN_CALLS, limits);
    budget_row("loop of bindings", SPIN_MATH, limits);
    budget_row("loop of small tables", SPIN_TABLES, limits);
    budget_row("loop of 4 KB strings", SPIN_STRINGS, limits);
    println!("the collector, in steps, with nothing else running:");
    for bytes in [64 << 10, 1 << 20, 8 << 20] {
        gc_row(bytes, None, Held::OneTable);
    }
    for size in [1, 4, 16] {
        gc_row(1 << 20, Some(size), Held::OneTable);
    }
    for size in [4, 16, 64] {
        gc_row(8 << 20, Some(size), Held::OneTable);
        gc_row(8 << 20, Some(size), Held::SmallTables);
    }
    gc_row(1 << 20, Some(16), Held::SmallTables);
}
