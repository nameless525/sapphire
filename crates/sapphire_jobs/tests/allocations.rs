//! Pins the per-call allocation cost the crate docs quote, so a bevy_tasks upgrade that changes
//! it is noticed. Own test binary: it installs the counting allocator and reads process totals.

use std::hint::black_box;

use sapphire_core::alloc::{self, CountingAllocator};

#[global_allocator]
static GLOBAL: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);

/// Allocations per call of `f`: the lowest average over 5 batches of 100 calls, after a
/// warm-up. Counts are process-wide, so anything else that allocates during a batch (a pool
/// thread waking up on a busy CI runner) only ever adds to it; the lowest batch is the calls'
/// own cost.
fn per_call(mut f: impl FnMut()) -> f64 {
    for _ in 0..10 {
        f();
    }
    let batch = |f: &mut dyn FnMut()| {
        let before = alloc::stats().total_allocations;
        for _ in 0..100 {
            f();
        }
        (alloc::stats().total_allocations - before) as f64 / 100.0
    };
    (0..5).map(|_| batch(&mut f)).fold(f64::INFINITY, f64::min)
}

#[test]
fn job_apis_allocate_about_three_plus_one_per_task() {
    sapphire_jobs::init(sapphire_jobs::JobConfig::default());
    let data = vec![1u32; 4096];
    let mut out = vec![0u32; 4096];

    let join = per_call(|| {
        black_box(sapphire_jobs::join(|| 1, || 2));
    });
    let four_chunks = per_call(|| {
        sapphire_jobs::par_for_each_chunk(&data, 1024, |_, chunk| {
            black_box(chunk);
        })
    });
    let two_chunks =
        per_call(|| sapphire_jobs::par_for_each_chunk_mut(&mut out, 2048, |_, chunk| chunk[0] += 1));
    let inline = per_call(|| {
        sapphire_jobs::par_for_each_chunk(&data, 4096, |_, chunk| {
            black_box(chunk);
        })
    });

    assert!(join <= 4.5, "join: {join}");
    assert!(four_chunks <= 7.5, "4 chunks: {four_chunks}");
    assert!(two_chunks <= 5.5, "2 chunks: {two_chunks}");
    assert_eq!(inline, 0.0, "a single chunk runs inline");
}
