//! Complete checked load and graph-freeze costs, including allocator attribution. Cooking and VM
//! startup happen outside the load timer; their costs are not claimed as improvements to load.
//!
//! Run the same source against baseline and candidate libraries. Every sample uses a fresh VM.

#![allow(clippy::print_stdout, reason = "benchmark raw samples")]

use std::alloc::System;

use sapphire_core::alloc::{self, CountingAllocator};
use sapphire_script::{Limits, ScriptVm, cook};
use sapphire_time::Stopwatch;

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

fn main() {
    for (shape, count) in
        [("wide", 8), ("wide", 64), ("wide", 1024), ("wide", 8192), ("shared", 8192), ("chain", 512)]
    {
        let source = match shape {
            "wide" => format!(
                "local t = {{}} for i = 1, {count} do t[i] = {{i}} end return {{hook = function() sapphire.mark(#t) end}}"
            ),
            "shared" => format!(
                "local t = {{}} local shared = {{}} for i = 1, {count} do t[i] = shared end return {{hook = function() sapphire.mark(#t) end}}"
            ),
            "chain" => format!(
                "local t = {{}} local root = t for i = 1, {count} do local child = {{}} t.child = child t = child end return {{hook = function() sapphire.mark(#root) end}}"
            ),
            _ => unreachable!(),
        };
        let bytes = cook(&source).expect("fixture cooks");
        for sample in 0..27 {
            let initializing = Stopwatch::start();
            let vm = ScriptVm::new(Limits { memory: 64 << 20, interrupts: 100_000, ..Limits::default() })
                .expect("starts");
            let host_before = alloc::thread_counts();
            let vm_before = alloc::vm_counts();
            let stopwatch = Stopwatch::start();
            let hooks = vm.load(&bytes, "=load-cost").expect("loads");
            let elapsed = stopwatch.elapsed();
            let startup_and_load = initializing.elapsed();
            let host_after = alloc::thread_counts();
            let vm_after = alloc::vm_counts();
            assert!(hooks.is_readonly());
            if sample >= 3 {
                println!(
                    "shape={shape} nodes={count} sample={} load_us={:.3} startup_and_load_us={:.3} host_allocations={} vm_allocations={} vm_bytes={} live_vm_bytes={}",
                    sample - 3,
                    elapsed.as_secs_f64() * 1e6,
                    startup_and_load.as_secs_f64() * 1e6,
                    host_after.allocations - host_before.allocations,
                    vm_after.allocations - vm_before.allocations,
                    vm_after.bytes - vm_before.bytes,
                    vm.used_memory()
                );
            }
        }
    }
}
