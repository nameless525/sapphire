//! Allocation tracking for development builds (spec §5.3, rule R6).
//!
//! Apps install [`EngineAllocator`] (mimalloc, counted; feature `mimalloc`) as their global
//! allocator:
//!
//! ```
//! # #[cfg(feature = "mimalloc")]
//! #[global_allocator]
//! static GLOBAL: sapphire_core::alloc::EngineAllocator = sapphire_core::alloc::engine_allocator();
//! # fn main() {}
//! ```
//!
//! or wrap any other allocator in a [`CountingAllocator`]:
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
//! Each allocation does four relaxed atomic read-modify-writes on a cache-line-aligned shard of
//! the counters, plus one thread-local `Cell` update; each deallocation does one relaxed atomic
//! subtraction. A few nanoseconds per call, however many threads allocate: where its stack is
//! picks a thread's shard, so threads rarely share one. [`stats`] sums the shards.
//!
//! One set of counters for the whole process cost far more than that. Ten threads building
//! pipelines through naga, which allocates throughout, spent most of their time passing its cache
//! line around: creating a renderer took 47 ms on the job pool, against 35 ms one after another
//! and 8 ms without shared counters, for 6 times the CPU (an M5, warm shader cache).
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
        record_dealloc(layout.size());
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller upholds `GlobalAlloc::realloc`'s contract for `ptr`, `layout` and
        // `new_size`.
        let new_ptr = unsafe { self.inner.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            // A realloc may move the block, so it counts as an allocation for rule R6.
            record_dealloc(layout.size());
            record_alloc(new_size);
        }
        new_ptr
    }
}

/// log2 of the shards: enough that a process's few dozen threads rarely meet.
const SHARD_BITS: u32 = 6;

/// log2 of the stretch of address space that picks a shard: half a megabyte, about the smallest
/// stack a thread gets, so two threads' stacks don't fall into one stretch.
const STACK_STRETCH_BITS: u32 = 19;

/// A thread's new bytes between looks at the total for [`PEAK_LIVE_BYTES`], so the peak can trail
/// the truth by this much per thread. Summing the shards reads other threads' cache lines, which
/// is what the shards are there to avoid, so it mustn't come often: a frame allocates less than
/// this in all.
const PEAK_CHECK_BYTES: usize = 1 << 20;

/// One shard of the process-wide counters, on its own cache line (128 bytes covers Apple
/// Silicon). Only their sums mean anything: a block freed on another thread comes off that
/// thread's shard, so one shard's `live_bytes` can go below zero, and wraps.
#[repr(align(128))]
struct Shard {
    allocations: AtomicU64,
    bytes_allocated: AtomicU64,
    live_bytes: AtomicUsize,
    /// Bytes allocated through this shard since it last looked at the total.
    since_peak_check: AtomicUsize,
}

static SHARDS: [Shard; 1 << SHARD_BITS] = [const {
    Shard {
        allocations: AtomicU64::new(0),
        bytes_allocated: AtomicU64::new(0),
        live_bytes: AtomicUsize::new(0),
        since_peak_check: AtomicUsize::new(0),
    }
}; 1 << SHARD_BITS];

/// The highest total any look at it has seen; [`stats`] adds what the total is now.
#[repr(align(128))]
struct PeakLiveBytes(AtomicUsize);

static PEAK_LIVE_BYTES: PeakLiveBytes = PeakLiveBytes(AtomicUsize::new(0));

/// The shard for this thread, by where its stack is, since a thread-local isn't safe here on every
/// target (see the module docs). A frame's address stays in one stretch of the stack or moves
/// to the next as calls nest, and a change of shard costs nothing but the odd shared line.
#[inline]
fn shard_index() -> usize {
    let on_the_stack = 0u8;
    let stretch = std::ptr::from_ref(&on_the_stack).addr() >> STACK_STRETCH_BITS;
    // Fibonacci hashing: stacks often sit at even spacings, which it spreads furthest apart.
    ((stretch as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (u64::BITS - SHARD_BITS)) as usize
}

#[inline]
fn record_alloc(size: usize) {
    let shard = &SHARDS[shard_index()];
    shard.allocations.fetch_add(1, Relaxed);
    shard.bytes_allocated.fetch_add(size as u64, Relaxed);
    shard.live_bytes.fetch_add(size, Relaxed);
    if shard.since_peak_check.fetch_add(size, Relaxed).saturating_add(size) >= PEAK_CHECK_BYTES {
        shard.since_peak_check.store(0, Relaxed);
        PEAK_LIVE_BYTES.0.fetch_max(live_bytes(), Relaxed);
    }
    per_thread::record(size);
}

#[inline]
fn record_dealloc(size: usize) {
    SHARDS[shard_index()].live_bytes.fetch_sub(size, Relaxed);
}

/// The shards' live bytes, summed. Read while other threads allocate, a block moving between
/// shards can make the sum come out below the truth, even below zero, which reads as zero.
fn live_bytes() -> usize {
    let sum = SHARDS.iter().fold(0usize, |sum, shard| sum.wrapping_add(shard.live_bytes.load(Relaxed)));
    sum.cast_signed().max(0).cast_unsigned()
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
    /// Highest `live_bytes` since startup or the last [`reset_peak`], to within a megabyte per
    /// allocating thread (a look at the total for every megabyte a thread allocates).
    pub peak_live_bytes: usize,
}

/// A snapshot of the process-wide counters: the shards' sums.
///
/// Each shard's fields are read separately with relaxed ordering, so a snapshot taken while other
/// threads allocate is approximate. Exact when they're quiet, however it was split among threads.
#[must_use]
pub fn stats() -> AllocStats {
    let mut stats = AllocStats { live_bytes: live_bytes(), ..AllocStats::default() };
    for shard in &SHARDS {
        stats.total_allocations += shard.allocations.load(Relaxed);
        stats.total_bytes += shard.bytes_allocated.load(Relaxed);
    }
    stats.peak_live_bytes = PEAK_LIVE_BYTES.0.load(Relaxed).max(stats.live_bytes);
    stats
}

/// Resets the peak to the current live bytes, for example at the start of a level.
pub fn reset_peak() {
    PEAK_LIVE_BYTES.0.store(live_bytes(), Relaxed);
}

/// Whether a [`CountingAllocator`] is the global allocator.
///
/// The count is nonzero once anything has allocated through one. Not every runtime allocates
/// before `main` (Windows doesn't), so while the count is still zero this allocates one byte to
/// find out.
#[must_use]
pub fn is_installed() -> bool {
    if stats().total_allocations == 0 {
        // `black_box` stops the optimizer from removing the allocation.
        drop(std::hint::black_box(Box::new(0u8)));
    }
    stats().total_allocations > 0
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

/// While alive, this thread's allocations count as the script VM's ([`vm_counts`]) rather than the
/// thread's ([`thread_counts`]). Luau's allocator is `mlua`'s, which calls the global allocator with no
/// way to give it another, so this is how gate 4 tells a hook's VM from everything else (spec §5.24): a
/// hook runs inside one, and a binding, which is Rust, leaves it through [`HostAllocations`] so that its
/// allocations still fail the gate. Does nothing where [`PER_THREAD_COUNTS`] is `false`.
#[derive(Debug)]
#[must_use = "allocations go back to the thread when this is dropped"]
pub struct VmAllocations {
    was: bool,
    _not_send: PhantomData<*const ()>,
}

impl VmAllocations {
    /// From now on this thread's allocations are the VM's.
    pub fn enter() -> Self {
        Self { was: per_thread::set_vm(true), _not_send: PhantomData }
    }
}

impl Drop for VmAllocations {
    fn drop(&mut self) {
        per_thread::set_vm(self.was);
    }
}

/// While alive, inside a [`VmAllocations`], this thread's allocations are the thread's again: what a
/// binding holds around its Rust.
#[derive(Debug)]
#[must_use = "allocations go back to the VM when this is dropped"]
pub struct HostAllocations {
    was: bool,
    _not_send: PhantomData<*const ()>,
}

impl HostAllocations {
    /// From now on this thread's allocations are the thread's.
    pub fn enter() -> Self {
        Self { was: per_thread::set_vm(false), _not_send: PhantomData }
    }
}

impl Drop for HostAllocations {
    fn drop(&mut self) {
        per_thread::set_vm(self.was);
    }
}

/// Cumulative counts of this thread's allocations inside a [`VmAllocations`]. Zero where
/// [`PER_THREAD_COUNTS`] is `false`.
#[must_use]
pub fn vm_counts() -> AllocCounts {
    per_thread::vm()
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
        static VM_COUNTS: Cell<AllocCounts> = const { Cell::new(AllocCounts { allocations: 0, bytes: 0 }) };
        static IN_VM: Cell<bool> = const { Cell::new(false) };
    }

    #[inline]
    pub(super) fn record(size: usize) {
        // `try_with` rather than `with`: an allocator must never panic.
        let in_vm = IN_VM.try_with(Cell::get).unwrap_or(false);
        let _ = (if in_vm { &VM_COUNTS } else { &COUNTS }).try_with(|counts| {
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

    pub(super) fn vm() -> AllocCounts {
        VM_COUNTS.try_with(Cell::get).unwrap_or_default()
    }

    /// Whether allocations are the VM's from here on, and what it was.
    #[inline]
    pub(super) fn set_vm(in_vm: bool) -> bool {
        IN_VM.try_with(|cell| cell.replace(in_vm)).unwrap_or(false)
    }
}

// Targets without native TLS: see the module docs for why thread-locals are off limits here.
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "windows", target_os = "linux")))]
mod per_thread {
    use super::{AllocCounts, stats};

    pub(super) const SUPPORTED: bool = false;

    #[inline]
    pub(super) fn record(_size: usize) {}

    pub(super) fn current() -> AllocCounts {
        let stats = stats();
        AllocCounts { allocations: stats.total_allocations, bytes: stats.total_bytes }
    }

    pub(super) fn vm() -> AllocCounts {
        AllocCounts::default()
    }

    #[inline]
    pub(super) fn set_vm(_in_vm: bool) -> bool {
        false
    }
}

/// The engine's global allocator: mimalloc, wrapped in a [`CountingAllocator`] so frame stats
/// and the R6 checks can see allocations. Android's dlopen-compatible TLS model is selected in
/// this crate's manifest, so apps need nothing platform-specific.
#[cfg(feature = "mimalloc")]
pub type EngineAllocator = CountingAllocator<mimalloc::MiMalloc>;

/// Constructs [`EngineAllocator`] for a `#[global_allocator]` static.
#[cfg(feature = "mimalloc")]
pub const fn engine_allocator() -> EngineAllocator {
    CountingAllocator::new(mimalloc::MiMalloc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_installed_is_false_without_a_counting_allocator() {
        // This unit-test binary keeps the default allocator, so even the probe goes uncounted.
        assert!(!is_installed());
        assert!(!is_installed(), "probing must not change the answer");
    }
}
