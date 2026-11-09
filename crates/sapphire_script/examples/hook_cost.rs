//! What a hook and a collection step cost, against spec §3.2's row (0.3 ms desktop, 0.5 ms mobile a
//! frame, hooks and the collector together).
//!
//! ```text
//! cargo run --release -p sapphire_script --example hook_cost
//! ```
//!
//! Each row fires a warm hook many times and reads the quietest tenth, the median and the slowest: the
//! slowest is mostly the scheduler while the machine's busy. The last rows play `arena-scripted`, a real
//! fight with every hook on it, and read the row itself: a step's hooks and the collector's step after it.

#![allow(clippy::print_stdout, reason = "a bench's output is what it's for")]

use std::alloc::System;
use std::time::Duration;

use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_gameplay::HookFrame;
use sapphire_gameplay::replay::{Player, Replay};
use sapphire_script::compile::{self, cook};
use sapphire_script::scenarios::SCRIPTED;
use sapphire_script::{CookedScript, Hook, HookEnd, Limits, ScriptVm, host_of};
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
    return { name = {phase, phase + 1}, adds = {}, slow = phase == 1 and 500 or 250 }
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
const SPIN_FIND: &str =
    "return { hook = function() local t = table.create(60000, 0) while true do table.find(t, 1) end end }";
const SPIN_FRONT: &str = "return { hook = function() local t = table.create(60000, 0) while true do table.remove(t, 1) table.insert(t, 1, 0) end end }";
const SPIN_FILL: &str = "return { hook = function() while true do local t = table.create(90000, 0) end end }";
const SPIN_LITERAL: &str = "local s = 'xxxxxxxxxxxxxxxx' return { hook = function() while true do local t = string.sub(s, 2) end end }";
const SPIN_TABLES: &str = "return { hook = function() while true do local t = {1, 2, 3, 4} end end }";

fn microseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1e6
}

fn load(vm: &ScriptVm, bytes: &CookedScript) -> Hook {
    vm.load(bytes, "=bench").expect("loads").hook("hook").expect("hook type").expect("has a hook")
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

fn fire_row(name: &str, bytes: &CookedScript, collect: Collector) {
    const WARM: usize = 2_000;
    const FIRES: usize = 20_000;
    let vm = ScriptVm::new(Limits { memory: 1 << 30, ..Limits::default() }).expect("starts");
    let hook = load(&vm, bytes);
    let mut frame = HookFrame::new(0);
    if matches!(collect, Collector::Running) {
        // SAFETY: this diagnostic only changes collector scheduling; it executes no binary bytecode.
        unsafe { vm.lua_for_tests() }.gc_restart();
    }
    let mut times = Vec::with_capacity(FIRES);
    let mut gc_times = Vec::with_capacity(FIRES);
    let mut allocations = 0;
    for fire in 0..WARM + FIRES {
        frame.tick = fire as u64;
        frame.truncate_queue(0);
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
    const WARM: usize = 2;
    const SAMPLES: usize = 24;
    let vm = ScriptVm::new(limits).expect("starts");
    let hook = load(&vm, &cook(source).expect("cooks"));
    let mut frame = HookFrame::new(0);
    let mut times = Vec::with_capacity(SAMPLES);
    for sample in 0..WARM + SAMPLES {
        let before = ScriptVm::interrupts();
        let stopwatch = Stopwatch::start();
        let end = vm.fire(&mut frame, &hook, ());
        let elapsed = stopwatch.elapsed();
        let count = ScriptVm::interrupts() - before;
        if sample >= WARM {
            println!(
                "budget_sample name={name:?} sample={} whole_hook_us={:.3} interrupts={count} end={end:.90?}",
                sample - WARM,
                microseconds(elapsed)
            );
            times.push((elapsed, count));
        }
        while !vm.gc_step().expect("collects") {}
    }
    times.sort_unstable();
    let observed = [50, 95, 99, 100].map(|p| microseconds(times[(times.len() * p).div_ceil(100) - 1].0));
    let (median, count) = times[(SAMPLES - 1) / 2];
    println!(
        "{name:<34} samples={SAMPLES} whole_hook_us[p50,p95,p99,max]={observed:?}; median sample {count} interrupts, {:.1} ns whole-hook/interrupt quotient",
        median.as_secs_f64() * 1e9 / count.max(1) as f64,
    );
}

/// What a heap is held in: a table is marked whole, so one big one is a step no step size can cut.
#[derive(Clone, Copy, Debug)]
enum Held {
    OneTable,
    SmallTables,
}

/// Whole observed cycles over a fixed live graph, optionally with newly unreachable tables before
/// each cycle. Cycle work is the sum of timed `gc_step` calls in that observed cycle, excluding the
/// harness work between calls. Raw samples preserve correlations; no independent minima are combined.
fn gc_row(bytes: usize, step_size_kb: Option<i32>, shape: Held, allocating: bool) {
    const CYCLES: usize = 24;
    let mut limits = Limits { memory: 256 << 20, interrupts: u64::MAX, ..Limits::default() };
    if let Some(size) = step_size_kb {
        limits.gc_step_kb = size;
    }
    let vm = ScriptVm::new(limits).expect("starts");
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
    // These direct calls deliberately measure heap/collection work, outside a fixed step. Both the
    // live graph and garbage have n one-element tables, with shape-specific container tables.
    let n = bytes / 64;
    // SAFETY: this diagnostic calls only the finite compiler-created graph function, without custom
    // conversions or binary execution, outside the simulation's fixed step.
    let keep: mlua::Value = unsafe { hook.function_for_tests() }.call(n).expect("live graph created");
    let garbage = allocating.then(|| {
        load(
            &vm,
            &cook("return {hook = function(n) local t = {} for i = 1, n do t[i] = {i} end end}")
                .expect("cooks"),
        )
    });
    let used = vm.used_memory();
    let workload = if allocating { "new-garbage" } else { "static-live-graph" };
    let mut cycles = Vec::with_capacity(CYCLES);
    let mut all_steps = Vec::new();
    for cycle in 0..CYCLES + 1 {
        if let Some(garbage) = &garbage {
            // SAFETY: this finite compiler-created function receives a built-in numeric argument;
            // the diagnostic intentionally measures collector work outside the fixed step.
            unsafe { garbage.function_for_tests() }.call::<()>(n).expect("garbage created");
        }
        let before_gc = vm.used_memory();
        let mut steps = Vec::new();
        loop {
            let stopwatch = Stopwatch::start();
            let finished = vm.gc_step().expect("collects");
            steps.push(stopwatch.elapsed());
            if finished {
                break;
            }
        }
        if cycle != 0 {
            let elapsed: Duration = steps.iter().sum();
            println!(
                "gc_sample workload={workload} shape={shape:?} n={n} new_tables={} step_kb={step_size_kb:?} cycle={} before_bytes={before_gc} after_bytes={} cycle_step_sum_us={:.3} steps_us={:?}",
                if allocating { n + 1 } else { 0 },
                cycle - 1,
                vm.used_memory(),
                microseconds(elapsed),
                steps.iter().copied().map(microseconds).collect::<Vec<_>>()
            );
            cycles.push(elapsed);
            all_steps.extend(steps);
        }
    }
    assert!(vm.collector_stopped(), "a step left the collector stopped");
    cycles.sort_unstable();
    all_steps.sort_unstable();
    let percentiles = |times: &[Duration]| {
        [50, 95, 99, 100].map(|p| microseconds(times[(times.len() * p).div_ceil(100) - 1]))
    };
    println!(
        "{shape:?} {workload} initial_live_bytes={used} n={n} step_kb={step_size_kb:?} cycles={} steps={} cycle_step_sum_us[p50,p95,p99,max]={:?} step_us[p50,p95,p99,max]={:?}",
        cycles.len(),
        all_steps.len(),
        percentiles(&cycles),
        percentiles(&all_steps)
    );
    drop(keep);
}

/// One step's whole allowance, as a crowd's hits on one tick might spend it: every event's hook a boss's.
fn crowd_row(events: usize) {
    const STEPS: usize = 5_000;
    let vm = ScriptVm::new(Limits::default()).expect("starts");
    let hook = load(&vm, &cook(BOSS_PHASE).expect("cooks"));
    let mut frame = HookFrame::new(0);
    let mut times = Vec::with_capacity(STEPS);
    for _ in 0..STEPS {
        let stopwatch = Stopwatch::start();
        for _ in 0..events {
            frame.truncate_queue(0);
            assert_eq!(vm.fire(&mut frame, &hook, ()), HookEnd::Returned);
        }
        times.push(stopwatch.elapsed());
        vm.gc_step().expect("collects");
    }
    let (low, median, high) = read(&mut times);
    println!("  {events} boss phases in one step: {low:.1} / {median:.1} / {high:.1} µs");
}

/// `arena-scripted`, whole: what each step with a hook spent on hooks, and each collector step.
fn fight_row() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/replays/arena-scripted.sreplay");
    let replay = Replay::decode(&std::fs::read(path).expect("committed")).expect("reads");
    let mut player = Player::new(replay, &SCRIPTED).expect("starts");
    let (mut hooks, mut collector) = (Vec::new(), Vec::new());
    let mut fired = 0;
    while player.step() {
        let host = host_of(player.simulation());
        let stats = host.stats();
        if stats.hooks_fired > fired {
            hooks.push(stats.last_step);
            fired = stats.hooks_fired;
        }
        collector.push(stats.last_collection);
    }
    let stats = host_of(player.simulation()).stats();
    let (steps, quiet) = (hooks.len(), player.simulation().tick() as usize - hooks.len());
    println!(
        "  {fired} hooks on {steps} steps, {quiet} steps with none; over budget {}",
        stats.over_budget_steps
    );
    let (low, median, high) = read(&mut hooks);
    println!("  a step's hooks: {low:.2} / {median:.2} / {high:.2} µs");
    let (low, median, high) = read(&mut collector);
    println!("  a collector step: {low:.2} / {median:.2} / {high:.2} µs");
}

fn main() {
    println!("hooks, one fire:");
    for (name, source) in [("empty", EMPTY), ("boss phase", BOSS_PHASE), ("careless", CARELESS)] {
        let cooked = cook(source).expect("cooks");
        fire_row(&format!("{name}, level 1, gc stopped"), &cooked, Collector::Stopped);
        fire_row(&format!("{name}, level 1, gc step each"), &cooked, Collector::StepEachFire);
        fire_row(&format!("{name}, level 1, gc running"), &cooked, Collector::Running);
        let level2 = compile::compile_unchecked(source, 2).expect("compiles");
        // SAFETY: these are the unmodified bytes just emitted by this crate's pinned compiler.
        let level2 = unsafe { CookedScript::from_trusted_bytes(level2) };
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
    budget_row("table.find over 60,000 slots", SPIN_FIND, limits);
    budget_row("table.insert at the front of 60,000", SPIN_FRONT, limits);
    budget_row("loop of table.create(90,000)", SPIN_FILL, limits);
    budget_row("loop of string.sub", SPIN_LITERAL, limits);
    println!("the collector, in steps, with nothing else running:");
    for allocating in [false, true] {
        for bytes in [64 << 10, 1 << 20, 8 << 20] {
            gc_row(bytes, None, Held::OneTable, allocating);
        }
        for size in [1, 4, 16] {
            gc_row(1 << 20, Some(size), Held::OneTable, allocating);
        }
        for size in [4, 16, 64] {
            gc_row(8 << 20, Some(size), Held::OneTable, allocating);
            gc_row(8 << 20, Some(size), Held::SmallTables, allocating);
        }
        gc_row(1 << 20, Some(16), Held::SmallTables, allocating);
    }
    println!("a step, against the row's 300 µs on a desktop and 500 µs on a phone:");
    for events in [1, 8, 64] {
        crowd_row(events);
    }
    fight_row();
}
