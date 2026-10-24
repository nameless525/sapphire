//! Allocation tracking for development builds (spec §5.3, rule R6).
//!
//! Apps wrap their real allocator (mimalloc in the engine; `System` here) in a
//! [`CountingAllocator`] in dev builds:
//!
//! ```
//! use sapphire_core::alloc::CountingAllocator;
//!
//! #[global_allocator]
//! static GLOBAL: CountingAllocator<std::alloc::System> = CountingAllocator::new(std::alloc::System);
//! # fn main() { assert!(sapphire_core::alloc::is_installed()); }
//! ```
//!
//! Library code then reads the counters through the free functions here, without needing access
//! to the app's static. If no counting allocator is installed every count stays zero, so tests
//! that assert "zero allocations" should first check [`is_installed`].
//!
//! # What rule R6 measures
//!
//! R6 means **zero heap allocations on the simulation thread inside the fixed step**. Combat
//! systems run single-threaded there by design (spec §4.4), so wrapping the fixed step in an
//! [`AllocScope`] (or [`count_in_scope`]) measures exactly that region: it counts allocations
//! made by the current thread, and nothing other threads do.
//!
//! Process-wide allocations per frame ([`stats`]) are a tracked budget, not a hard zero: the
//! renderer, audio and the job system allocate on other threads (wgpu alone does about 130 per
//! frame today).
//!
//! A thread waiting on the job system runs other pool tasks while it waits, and their
//! allocations happen on, and count against, that thread. Conversely, tasks the scoped code
//! hands to other workers are not counted. So code inside a zero-allocation scope must not call
//! into `sapphire_jobs` at all; the job APIs allocate per call anyway.
//!
//! # Overhead
//!
//! Each allocation does three relaxed atomic read-modify-writes and one relaxed load on a single
//! cache-line-aligned block, plus one thread-local `Cell` update; each deallocation does one
//! relaxed atomic subtraction. Uncontended that is a few nanoseconds per call. When many threads
//! allocate at once the shared cache line bounces between cores and the cost can reach tens of
//! nanoseconds per call, which is why shipping builds use the bare allocator.
//!
//! # Targets without native thread-local storage
//!
//! Per-thread counting needs thread-locals that never allocate. That holds only where Rust uses
//! native TLS, so it is enabled on an allowlist of targets known to have it: macOS, iOS, Windows
//! and Linux. Elsewhere (Android today, whose Rust target uses emulated TLS), `thread_local!`
//! lazily allocates its slot through the global allocator, which would re-enter this allocator
//! and recurse without bound. There the allocator never touches thread-locals, and
//! [`thread_counts`] reports process-wide totals instead (see [`PER_THREAD_COUNTS`]). Scoped
//! counts are then an upper bound: a zero assertion cannot pass falsely, but other threads'
//! allocations can make it fail. The R6 replay suite runs on desktop, so nothing depends on
//! that path being exact.
//!
//! The target allowlist is an accepted exception to rule R7 (ADR 0001): the TLS model is a
//! property of the compile target that no runtime query can provide.

use std::alloc::{GlobalAlloc, Layout};
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

/// A [`GlobalAlloc`] wrapper that counts allocations, live bytes and the live-byte peak.
///
/// All instances share one set of counters, because a process has only one global allocator.
#[derive(Debug, Default)]
pub struct CountingAllocator<A> {
    inner: A,
}

impl<A> CountingAllocator<A> {
    /// Wraps `inner`. `const` so it can initialize a `#[global_allocator]` static.
    pub const fn new(inner: A) -> Self {
        Self { inner }
    }

    /// The wrapped allocator.
    pub const fn inner(&self) -> &A {
        &self.inner
    }
}

// SAFETY: every method forwards to `inner` with the caller's arguments unchanged and returns its
// result unchanged; the bookkeeping only touches atomics and a const-initialized, drop-free
// thread-local, neither of which allocates or unwinds.
unsafe impl<A: GlobalAlloc> GlobalAlloc for CountingAllocator<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract for `layout`.
        let ptr = unsafe { self.inner.alloc(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::alloc_zeroed`'s contract for `layout`.
        let ptr = unsafe { self.inner.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_alloc(layout.size());
        }
        ptr
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller guarantees `ptr` came from this allocator with `layout`.
        unsafe { self.inner.dealloc(ptr, layout) };
        COUNTERS.live_bytes.fetch_sub(layout.size(), Relaxed);
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract for `ptr`, `layout` and
        // `new_size`.
        let new_ptr = unsafe { self.inner.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // A realloc may move the block, so it counts as an allocation for rule R6.
            COUNTERS.live_bytes.fetch_sub(layout.size(), Relaxed);
            record_alloc(new_size);
        }
        new_ptr
    }
}

/// Process-wide counters, on their own cache line (128 bytes covers Apple Silicon) so that
/// unrelated statics never share it.
#[repr(align(128))]
struct Counters {
    allocations: AtomicU64,
    bytes_allocated: AtomicU64,
    live_bytes: AtomicUsize,
    peak_live_bytes: AtomicUsize,
}

static COUNTERS: Counters = Counters {
    allocations: AtomicU64::new(0),
    bytes_allocated: AtomicU64::new(0),
    live_bytes: AtomicUsize::new(0),
    peak_live_bytes: AtomicUsize::new(0),
};

#[inline]
fn record_alloc(size: usize) {
    COUNTERS.allocations.fetch_add(1, Relaxed);
    COUNTERS.bytes_allocated.fetch_add(size as u64, Relaxed);
    let live = COUNTERS.live_bytes.fetch_add(size, Relaxed).wrapping_add(size);
    // Load first: the peak rarely moves, and a load does not take the line exclusively.
    if live > COUNTERS.peak_live_bytes.load(Relaxed) {
        COUNTERS.peak_live_bytes.fetch_max(live, Relaxed);
    }
    per_thread::record(size);
}

/// Whether [`thread_counts`] and [`AllocScope`] count only the current thread on this target.
/// `false` on targets without native thread-locals, where they fall back to process-wide
/// totals (see the module docs).
pub const PER_THREAD_COUNTS: bool = per_thread::SUPPORTED;

/// Process-wide allocation statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocStats {
    /// Allocations since startup, including reallocations.
    pub total_allocations: u64,
    /// Bytes requested since startup.
    pub total_bytes: u64,
    /// Bytes currently allocated.
    pub live_bytes: usize,
    /// Highest `live_bytes` since startup or the last [`reset_peak`].
    pub peak_live_bytes: usize,
}

/// A snapshot of the process-wide counters.
///
/// Each field is read separately with relaxed ordering, so a snapshot taken while other threads
/// allocate is approximate (for example `peak_live_bytes` may briefly trail `live_bytes`).
#[must_use]
pub fn stats() -> AllocStats {
    AllocStats {
        total_allocations: COUNTERS.allocations.load(Relaxed),
        total_bytes: COUNTERS.bytes_allocated.load(Relaxed),
        live_bytes: COUNTERS.live_bytes.load(Relaxed),
        peak_live_bytes: COUNTERS.peak_live_bytes.load(Relaxed),
    }
}

/// Resets the peak to the current live bytes, for example at the start of a level.
pub fn reset_peak() {
    COUNTERS.peak_live_bytes.store(COUNTERS.live_bytes.load(Relaxed), Relaxed);
}

/// Whether a [`CountingAllocator`] is the global allocator. Any Rust program allocates before
/// `main`, so a zero count means no counting allocator is installed.
#[must_use]
pub fn is_installed() -> bool {
    COUNTERS.allocations.load(Relaxed) > 0
}

/// Allocation counts for one thread (or, where [`PER_THREAD_COUNTS`] is `false`, the process).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AllocCounts {
    /// Number of allocations, including reallocations.
    pub allocations: u64,
    /// Bytes requested by those allocations.
    pub bytes: u64,
}

impl AllocCounts {
    fn since(self, start: Self) -> Self {
        Self {
            allocations: self.allocations.wrapping_sub(start.allocations),
            bytes: self.bytes.wrapping_sub(start.bytes),
        }
    }
}

/// Cumulative counts for the current thread since it started.
#[must_use]
pub fn thread_counts() -> AllocCounts {
    per_thread::current()
}

/// Measures allocations made on the current thread while it is alive.
///
/// Not `Send`: the counts belong to the thread that created the scope.
///
/// ```
/// use sapphire_core::alloc::AllocScope;
///
/// let scope = AllocScope::begin();
/// let sum: u32 = [1, 2, 3].iter().sum();
/// assert_eq!(scope.counts().allocations, 0);
/// # let _ = sum;
/// ```
#[derive(Debug)]
#[must_use = "an AllocScope measures nothing unless its counts are read"]
pub struct AllocScope {
    start: AllocCounts,
    _not_send: PhantomData<*const ()>,
}

impl AllocScope {
    /// Starts measuring from now.
    pub fn begin() -> Self {
        Self { start: thread_counts(), _not_send: PhantomData }
    }

    /// Allocations on this thread since [`AllocScope::begin`].
    #[must_use]
    pub fn counts(&self) -> AllocCounts {
        thread_counts().since(self.start)
    }
}

/// Runs `f` and returns its result with the allocations it made on the current thread.
pub fn count_in_scope<R>(f: impl FnOnce() -> R) -> (R, AllocCounts) {
    let scope = AllocScope::begin();
    let result = f();
    (result, scope.counts())
}

#[cfg(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux"))]
mod per_thread {
    use std::cell::Cell;

    use super::AllocCounts;

    pub(super) const SUPPORTED: bool = true;

    thread_local! {
        // Const-initialized and drop-free, so on targets with native TLS accessing it never
        // allocates or registers a destructor, which makes it safe inside the allocator.
        static COUNTS: Cell<AllocCounts> = const { Cell::new(AllocCounts { allocations: 0, bytes: 0 }) };
    }

    #[inline]
    pub(super) fn record(size: usize) {
        // `try_with` rather than `with`: an allocator must never panic.
        let _ = COUNTS.try_with(|counts| {
            let current = counts.get();
            counts.set(AllocCounts {
                allocations: current.allocations.wrapping_add(1),
                bytes: current.bytes.wrapping_add(size as u64),
            });
        });
    }

    pub(super) fn current() -> AllocCounts {
        COUNTS.try_with(Cell::get).unwrap_or_default()
    }
}

// Targets without native TLS: see the module docs for why thread-locals are off limits here.
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux")))]
mod per_thread {
    use std::sync::atomic::Ordering::Relaxed;

    use super::{AllocCounts, COUNTERS};

    pub(super) const SUPPORTED: bool = false;

    #[inline]
    pub(super) fn record(_size: usize) {}

    pub(super) fn current() -> AllocCounts {
        AllocCounts {
            allocations: COUNTERS.allocations.load(Relaxed),
            bytes: COUNTERS.bytes_allocated.load(Relaxed),
        }
    }
}
