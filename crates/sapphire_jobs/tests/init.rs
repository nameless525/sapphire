//! `init` configures process-global pools, so it gets its own test binary with one test.

use bevy_tasks::{AsyncComputeTaskPool, ComputeTaskPool, block_on};
use sapphire_jobs::JobConfig;

fn worker_name(pool: &bevy_tasks::TaskPool) -> String {
    // A detached `spawn` runs only on the pool's own threads, never on the caller.
    block_on(pool.spawn(async { std::thread::current().name().map(str::to_owned) }))
        .expect("pool threads are named")
}

#[test]
fn init_configures_once_and_then_returns_the_same_system() {
    let config = JobConfig { compute_threads: Some(3), async_compute_threads: Some(2) };
    let first = sapphire_jobs::init(config);
    assert_eq!(first.config(), config);
    assert_eq!(first.compute_pool().thread_num(), 3);
    assert_eq!(first.async_compute_pool().thread_num(), 2);

    // The engine pools are bevy's globals, which bevy_ecs will use too.
    assert!(std::ptr::eq(first.compute_pool(), &**ComputeTaskPool::get()));
    assert!(std::ptr::eq(first.async_compute_pool(), &**AsyncComputeTaskPool::get()));
    assert!(worker_name(first.compute_pool()).starts_with("sapphire-worker ("));
    assert!(worker_name(first.async_compute_pool()).starts_with("sapphire-async ("));

    // A second init with another config returns the running system unchanged.
    let second = sapphire_jobs::init(JobConfig { compute_threads: Some(7), async_compute_threads: None });
    assert!(std::ptr::eq(first, second));
    assert_eq!(second.config(), config);
    assert_eq!(second.compute_pool().thread_num(), 3);
    assert!(std::ptr::eq(first, sapphire_jobs::system()));

    // And the job APIs run on it.
    let (a, b) = sapphire_jobs::join(|| 1, || 2);
    assert_eq!(a + b, 3);
}
