//! Installs `CountingAllocator` as this test binary's global allocator.

use std::alloc::System;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};

use sapphire_core::alloc::{self, AllocScope, CountingAllocator};

#[global_allocator]
static GLOBAL: CountingAllocator<System> = CountingAllocator::new(System);

#[test]
fn is_installed() {
    assert!(alloc::is_installed());
}

#[test]
fn counts_allocations_on_this_thread() {
    let (vec, counts) = alloc::count_in_scope(|| black_box(Vec::<u8>::with_capacity(100)));
    assert_eq!(counts.allocations, 1);
    assert_eq!(counts.bytes, 100);
    drop(vec);

    let (_, counts) = alloc::count_in_scope(|| black_box(black_box(2) + black_box(3)));
    assert_eq!(counts.allocations, 0);
    assert_eq!(counts.bytes, 0);
}

#[test]
fn growth_counts_reallocations() {
    let mut vec = Vec::<u64>::with_capacity(1);
    let scope = AllocScope::begin();
    for i in 0..1000 {
        vec.push(black_box(i));
    }
    let counts = scope.counts();
    assert!(counts.allocations >= 5, "{counts:?}");
    assert!(counts.bytes >= 1000 * 8, "{counts:?}");
}

#[test]
fn other_threads_are_not_counted() {
    let go = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    std::thread::scope(|s| {
        // Spawn before the scope begins: spawning itself allocates on this thread.
        let worker = s.spawn(|| {
            while !go.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            let vec = black_box(vec![0u8; 4096]);
            done.store(true, Ordering::Release);
            vec.len()
        });
        let scope = AllocScope::begin();
        go.store(true, Ordering::Release);
        while !done.load(Ordering::Acquire) {
            std::hint::spin_loop();
        }
        assert_eq!(scope.counts().allocations, 0);
        assert_eq!(worker.join().expect("worker"), 4096);
    });
}

#[test]
fn process_stats_track_live_and_peak() {
    let before = alloc::stats();
    let block = black_box(vec![0u8; 1 << 20]);
    let during = alloc::stats();
    assert!(during.total_allocations > before.total_allocations);
    assert!(during.total_bytes >= before.total_bytes + (1 << 20));
    assert!(during.peak_live_bytes >= 1 << 20);
    drop(block);
    alloc::reset_peak();
    // Other tests allocate concurrently, but nowhere near the megabyte just freed.
    assert!(alloc::stats().peak_live_bytes < during.peak_live_bytes);
}
