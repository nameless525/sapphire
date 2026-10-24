//! The engine's single task pool (spec §4.3, §5.5).
//!
//! Owns the worker threads: `bevy_tasks`' `ComputeTaskPool`, sized for the device's cores, and
//! an `AsyncComputeTaskPool` for work that may span frames. bevy_ecs's executor uses the same
//! compute pool, so the engine never runs two pools that fight over cores (rayon is for tools
//! only). Waiting on a scope or a [`join`] runs other tasks on the waiting thread instead of
//! blocking it.
//!
//! Does not own: the IO thread (`sapphire_platform`, spec §4.3). `bevy_tasks`' `IoTaskPool` is
//! left uninitialized. Named Tracy zones per task and thread priorities arrive with the
//! profiling integration in M1.
//!
//! # Allocation
//!
//! Every call allocates: [`scope`], [`join`] and the parallel loops cost about 3 heap
//! allocations plus 1 per spawned task or chunk (measured with bevy_tasks 0.20: `join` 4,
//! `par_for_each_chunk` over 4 chunks 7). Rule R6 forbids allocation on the simulation thread
//! inside the fixed step, so none of these belong there; combat systems run single-threaded in
//! that region by design (spec §4.4), and the wide parallel passes (animation, culling, extract)
//! run outside it. Pooled task storage that would lift this restriction is future work.
//!
//! While a thread waits for a scope or a `join`, it runs other pool tasks, including ones
//! unrelated to its own work; their allocations count against that thread in
//! `sapphire_core::alloc::AllocScope`.

pub mod topology;

use std::sync::OnceLock;

use bevy_tasks::{AsyncComputeTaskPool, ComputeTaskPool, TaskPool, TaskPoolBuilder};

use sapphire_core::target::FormFactor;

pub use topology::CpuTopology;

/// How [`init`] sizes the pools. `None` fields use the defaults from [`CpuTopology`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobConfig {
    /// Compute workers. Default: physical cores − 1 on desktop (at most 16), performance
    /// cores − 1 on mobile; at least 1.
    pub compute_threads: Option<usize>,
    /// Workers for long-running tasks. Default: 2 on mobile, physical cores / 4 (1–4) on desktop.
    pub async_compute_threads: Option<usize>,
}

/// The running job system. Lives for the rest of the process once created.
#[derive(Debug)]
pub struct JobSystem {
    config: JobConfig,
    topology: CpuTopology,
    compute: &'static TaskPool,
    async_compute: &'static TaskPool,
}

impl JobSystem {
    /// The configuration the system was created with.
    #[must_use]
    pub fn config(&self) -> JobConfig {
        self.config
    }

    /// The CPU topology detected at startup.
    #[must_use]
    pub fn topology(&self) -> CpuTopology {
        self.topology
    }

    /// The pool used by [`scope`], [`join`] and the parallel loops (and by bevy_ecs).
    #[must_use]
    pub fn compute_pool(&self) -> &'static TaskPool {
        self.compute
    }

    /// The pool for tasks that may run across frames, such as decoding or navmesh queries.
    #[must_use]
    pub fn async_compute_pool(&self) -> &'static TaskPool {
        self.async_compute
    }

    fn start(config: JobConfig) -> Self {
        let topology = CpuTopology::detect();
        let form_factor = FormFactor::CURRENT;
        let compute_threads =
            config.compute_threads.unwrap_or_else(|| topology.default_compute_workers(form_factor)).max(1);
        let async_threads = config
            .async_compute_threads
            .unwrap_or_else(|| topology.default_async_compute_workers(form_factor))
            .max(1);

        let mut created = [false; 2];
        let compute: &'static TaskPool = ComputeTaskPool::get_or_init(|| {
            created[0] = true;
            build_pool("sapphire-worker", compute_threads)
        });
        let async_compute: &'static TaskPool = AsyncComputeTaskPool::get_or_init(|| {
            created[1] = true;
            build_pool("sapphire-async", async_threads)
        });
        for (created, name, pool) in
            [(created[0], "ComputeTaskPool", compute), (created[1], "AsyncComputeTaskPool", async_compute)]
        {
            if !created {
                tracing::warn!(
                    target: "sapphire::jobs",
                    pool = name,
                    threads = pool.thread_num(),
                    "pool was created before sapphire_jobs started; using it as is"
                );
            }
        }
        tracing::info!(
            target: "sapphire::jobs",
            ?topology,
            ?form_factor,
            compute_threads = compute.thread_num(),
            async_compute_threads = async_compute.thread_num(),
            "job system started"
        );
        Self { config, topology, compute, async_compute }
    }
}

/// Threads are named `<prefix> (<index>)` so they are recognizable in debuggers and profilers.
fn build_pool(thread_prefix: &str, threads: usize) -> TaskPool {
    TaskPoolBuilder::new().num_threads(threads).thread_name(thread_prefix.to_owned()).build()
}

static SYSTEM: OnceLock<JobSystem> = OnceLock::new();

/// Starts the job system, or returns the one already running.
///
/// Call once at startup, before anything spawns work. A later call (or one made after a job
/// API has started the system with [`JobConfig::default`]) returns the existing system
/// unchanged, logging a warning if `config` differs.
pub fn init(config: JobConfig) -> &'static JobSystem {
    let mut created = false;
    let system = SYSTEM.get_or_init(|| {
        created = true;
        JobSystem::start(config)
    });
    if !created && system.config != config {
        tracing::warn!(
            target: "sapphire::jobs",
            requested = ?config,
            running = ?system.config,
            "sapphire_jobs::init called again with a different config; keeping the running system"
        );
    }
    system
}

/// The running job system, started with [`JobConfig::default`] if [`init`] has not run yet.
pub fn system() -> &'static JobSystem {
    SYSTEM.get_or_init(|| JobSystem::start(JobConfig::default()))
}

/// Spawns tasks that run on the compute pool, from inside a [`scope`] call.
///
/// Tasks may borrow anything that outlives the `scope` call. Results come back in spawn order
/// for tasks spawned directly by the closure passed to [`scope`].
pub struct Scope<'scope, 'env, T> {
    // `bevy_tasks::Scope` is invariant in both lifetimes, and so is this wrapper.
    inner: &'scope bevy_tasks::Scope<'scope, 'env, T>,
}

impl<'scope, 'env, T: Send + 'scope> Scope<'scope, 'env, T> {
    /// Runs `task` on a worker (or on the thread waiting for the scope).
    pub fn spawn(&self, task: impl FnOnce() -> T + Send + 'scope) {
        self.inner.spawn(async move { task() });
    }
}

impl<T> Clone for Scope<'_, '_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Scope<'_, '_, T> {}

/// Runs `f`, which spawns tasks through the [`Scope`] it is given, and returns once every
/// task has finished, with their results. The calling thread runs tasks while it waits.
/// Allocates per call and per task (see the crate docs), so keep it out of the fixed step.
///
/// ```
/// let data = [1, 2, 3, 4];
/// let sums = sapphire_jobs::scope(|s| {
///     for half in data.chunks(2) {
///         s.spawn(move || half.iter().sum::<i32>());
///     }
/// });
/// assert_eq!(sums, [3, 7]);
/// ```
///
/// # Panics
///
/// Re-raises the panic of any task that panicked.
pub fn scope<'env, F, T>(f: F) -> Vec<T>
where
    F: for<'scope> FnOnce(Scope<'scope, 'env, T>),
    T: Send + 'static,
{
    system().compute.scope(|inner| f(Scope { inner }))
}

/// Runs `a` on the calling thread and `b` on the pool, potentially in parallel, and returns
/// both results. If `a` panics, the panic propagates only after `b` has finished, so `b` never
/// outlives the data it borrows. Allocates per call (see the crate docs).
///
/// # Panics
///
/// Re-raises a panic from either closure.
pub fn join<A, B, RA, RB>(a: A, b: B) -> (RA, RB)
where
    A: FnOnce() -> RA,
    B: FnOnce() -> RB + Send,
    RB: Send,
{
    let mut result_a = None;
    let mut result_b = None;
    system().compute.scope(|s| {
        s.spawn(async { result_b = Some(b()) });
        result_a = Some(a());
    });
    (result_a.expect("`a` ran on the calling thread"), result_b.expect("scope waits for every spawned task"))
}

/// Calls `f(start, chunk)` for consecutive chunks of `slice` in parallel, where `start` is the
/// index of the chunk's first element. Every element is visited exactly once; chunk order is
/// unspecified. A slice that fits in one chunk runs inline without allocating; otherwise each
/// call allocates about 3 times plus once per chunk (see the crate docs).
///
/// # Panics
///
/// If `chunk_size` is 0, or re-raises a panic from `f`.
pub fn par_for_each_chunk<T, F>(slice: &[T], chunk_size: usize, f: F)
where
    T: Sync,
    F: Fn(usize, &[T]) + Sync,
{
    assert!(chunk_size > 0, "chunk_size must be positive");
    if slice.len() <= chunk_size {
        if !slice.is_empty() {
            f(0, slice);
        }
        return;
    }
    let f = &f;
    system().compute.scope(|s| {
        for (index, chunk) in slice.chunks(chunk_size).enumerate() {
            s.spawn(async move { f(index * chunk_size, chunk) });
        }
    });
}

/// Like [`par_for_each_chunk`], but each call gets exclusive access to its chunk.
///
/// # Panics
///
/// If `chunk_size` is 0, or re-raises a panic from `f`.
pub fn par_for_each_chunk_mut<T, F>(slice: &mut [T], chunk_size: usize, f: F)
where
    T: Send,
    F: Fn(usize, &mut [T]) + Sync,
{
    assert!(chunk_size > 0, "chunk_size must be positive");
    if slice.len() <= chunk_size {
        if !slice.is_empty() {
            f(0, slice);
        }
        return;
    }
    let f = &f;
    system().compute.scope(|s| {
        for (index, chunk) in slice.chunks_mut(chunk_size).enumerate() {
            s.spawn(async move { f(index * chunk_size, chunk) });
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn par_for_each_chunk_visits_every_item_once() {
        let counts: Vec<AtomicU32> = (0..10_007).map(|_| AtomicU32::new(0)).collect();
        for chunk_size in [1, 7, 64, 1000, 10_007, 50_000] {
            par_for_each_chunk(&counts, chunk_size, |start, chunk| {
                assert!(chunk.len() <= chunk_size);
                for (offset, count) in chunk.iter().enumerate() {
                    // `start` must locate the chunk within the slice.
                    assert!(std::ptr::eq(count, &counts[start + offset]));
                    count.fetch_add(1, Ordering::Relaxed);
                }
            });
            assert!(
                counts.iter().all(|count| count.swap(0, Ordering::Relaxed) == 1),
                "chunk size {chunk_size}"
            );
        }
    }

    #[test]
    fn par_for_each_chunk_mut_writes_every_item_once() {
        for chunk_size in [1, 3, 256, 4096, 9999] {
            let mut items = vec![usize::MAX; 4096];
            par_for_each_chunk_mut(&mut items, chunk_size, |start, chunk| {
                for (offset, item) in chunk.iter_mut().enumerate() {
                    assert_eq!(*item, usize::MAX, "visited twice");
                    *item = start + offset;
                }
            });
            assert!(items.iter().enumerate().all(|(index, &item)| item == index), "chunk size {chunk_size}");
        }
    }

    #[test]
    fn empty_slices_never_call_f() {
        let calls = AtomicUsize::new(0);
        par_for_each_chunk(&[] as &[u8], 4, |_, _| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        par_for_each_chunk_mut(&mut [] as &mut [u8], 4, |_, _| {
            calls.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    #[should_panic(expected = "chunk_size must be positive")]
    fn zero_chunk_size_panics() {
        par_for_each_chunk(&[1, 2, 3], 0, |_, _| {});
    }

    #[test]
    fn join_returns_both_results_and_borrows() {
        let left = vec![1u64; 1000];
        let right = vec![2u64; 1000];
        let (a, b) = join(|| left.iter().sum::<u64>(), || right.iter().sum::<u64>());
        assert_eq!((a, b), (1000, 2000));
    }

    #[test]
    fn nested_join_does_not_deadlock() {
        fn sum(values: &[u64]) -> u64 {
            if values.len() <= 64 {
                return values.iter().sum();
            }
            let (left, right) = values.split_at(values.len() / 2);
            let (a, b) = join(|| sum(left), || sum(right));
            a + b
        }
        let values: Vec<u64> = (0..100_000).collect();
        assert_eq!(sum(&values), values.iter().sum());
    }

    #[test]
    fn scope_returns_results_in_spawn_order() {
        let data: Vec<u32> = (0..100).collect();
        let results = scope(|s| {
            for chunk in data.chunks(10) {
                s.spawn(move || chunk.iter().sum::<u32>());
            }
        });
        let expected: Vec<u32> = data.chunks(10).map(|chunk| chunk.iter().sum()).collect();
        assert_eq!(results, expected);
    }

    #[test]
    fn tasks_can_spawn_tasks() {
        let total = AtomicUsize::new(0);
        let results = scope(|s| {
            for _ in 0..4 {
                let total = &total;
                s.spawn(move || {
                    for _ in 0..4 {
                        s.spawn(move || {
                            total.fetch_add(1, Ordering::Relaxed);
                        });
                    }
                    total.fetch_add(1, Ordering::Relaxed);
                });
            }
        });
        assert_eq!(results.len(), 20);
        assert_eq!(total.load(Ordering::Relaxed), 20);
    }

    #[test]
    #[should_panic(expected = "task failed")]
    fn task_panics_propagate() {
        scope::<_, ()>(|s| s.spawn(|| panic!("task failed")));
    }
}
