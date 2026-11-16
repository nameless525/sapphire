//! What a thread, the process and the machine's cores are doing, as the OS counts it (M5 X3): what a lane
//! reads around a frame it traces, to tell a frame whose thread computed from one whose thread was off its
//! core (waiting on a lock, a file or a driver, or starved by other work), a read the file cache answered from
//! one that went to the disk, and the machine's cores busy from idle. The one place that knows how each OS
//! says it.
//!
//! | OS | Thread's CPU time | Reads | Cores busy |
//! |---|---|---|---|
//! | macOS, iOS | `CLOCK_THREAD_CPUTIME_ID` | `proc_pid_rusage`'s bytes read from disk and page-ins | `host_processor_info`'s ticks per CPU, by kind ([`core_layout`](crate::core_layout)) |
//! | Linux, Android | `CLOCK_THREAD_CPUTIME_ID` | `/proc/self/io`'s `read_bytes` and `getrusage`'s major faults | `None` |
//! | Others | `None` | `None` | `None` |
//!
//! Every value adds up from some start and is read twice, around what it measures. A read is a call or two
//! into the kernel and allocates nothing, except Linux's reads, which read a file: nothing reads them unless
//! something traces.

use std::time::Duration;

/// The calling thread's CPU time, user and system, since it started: beside the wall time of what it did, how
/// long it was off its core. `None` where the OS doesn't say.
#[must_use]
pub fn thread_cpu_time() -> Option<Duration> {
    os::thread_cpu_time()
}

/// What the process has read from storage since it started, as its OS counts it: reads the file cache
/// answered aren't in it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessReads {
    /// Bytes read from the disk.
    pub disk_bytes: u64,
    /// Pages faulted in from storage: a mapped file's pages, or memory paged back in.
    pub page_ins: u64,
}

impl ProcessReads {
    /// What was read between `before` and this read.
    #[must_use]
    pub fn since(self, before: Self) -> Self {
        Self {
            disk_bytes: self.disk_bytes.saturating_sub(before.disk_bytes),
            page_ins: self.page_ins.saturating_sub(before.page_ins),
        }
    }
}

/// What the process has read from storage. `None` where the OS doesn't say.
#[must_use]
pub fn process_reads() -> Option<ProcessReads> {
    os::process_reads()
}

/// Scheduler ticks across some cores: those they spent busy (user, system and low-priority work) and all of
/// them, idle among them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ticks {
    pub busy: u64,
    pub all: u64,
}

impl Ticks {
    /// The ticks between `before` and this read.
    #[must_use]
    pub fn since(self, before: Self) -> Self {
        Self { busy: self.busy.saturating_sub(before.busy), all: self.all.saturating_sub(before.all) }
    }

    /// The share of them that were busy, 0 to 1, or `None` with no tick to judge by: a kernel's tick is
    /// 10 ms, so a few milliseconds may hold none.
    #[must_use]
    pub fn busy_share(self) -> Option<f32> {
        (self.all > 0).then(|| self.busy as f32 / self.all as f32)
    }
}

/// The machine's cores' ticks since it started, by kind. On a CPU of one kind every core counts as a
/// performance core.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreTicks {
    pub performance: Ticks,
    pub efficiency: Ticks,
}

impl CoreTicks {
    /// The ticks between `before` and this read.
    #[must_use]
    pub fn since(self, before: Self) -> Self {
        Self {
            performance: self.performance.since(before.performance),
            efficiency: self.efficiency.since(before.efficiency),
        }
    }
}

/// The machine's cores' ticks, by kind. `None` where the OS doesn't say.
#[must_use]
pub fn core_ticks() -> Option<CoreTicks> {
    os::core_ticks()
}

/// `CLOCK_THREAD_CPUTIME_ID`, on every Unix the engine ships on.
#[cfg(unix)]
fn unix_thread_cpu_time() -> Option<Duration> {
    let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: `now` is a `timespec` the call writes; the clock names the calling thread, which is alive.
    if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut now) } != 0 {
        return None;
    }
    Some(Duration::new(u64::try_from(now.tv_sec).ok()?, u32::try_from(now.tv_nsec).ok()?))
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod os {
    use std::ffi::{c_int, c_void};
    use std::sync::OnceLock;
    use std::time::Duration;

    use mach2::kern_return::KERN_SUCCESS;
    use mach2::mach_init::mach_host_self;
    use mach2::traps::mach_task_self;

    use super::{CoreTicks, ProcessReads};
    use crate::cores::core_layout;

    unsafe extern "C" {
        // `<libproc.h>`, on macOS and iOS alike (the `libc` crate declares it for macOS alone).
        fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut *mut c_void) -> c_int;
    }

    pub(super) fn thread_cpu_time() -> Option<Duration> {
        super::unix_thread_cpu_time()
    }

    pub(super) fn process_reads() -> Option<ProcessReads> {
        // SAFETY: a struct of plain integers, which all-zero is a value of.
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        // SAFETY: the flavor names the struct `info` is, which the call fills; the process is this one.
        let result = unsafe {
            proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V2, (&raw mut info).cast::<*mut c_void>())
        };
        (result == 0)
            .then_some(ProcessReads { disk_bytes: info.ri_diskio_bytesread, page_ins: info.ri_pageins })
    }

    pub(super) fn core_ticks() -> Option<CoreTicks> {
        // One send right, kept: each call of `mach_host_self` adds a reference to it.
        static HOST: OnceLock<libc::host_t> = OnceLock::new();
        // SAFETY: no arguments; it names this machine's host port.
        let host = *HOST.get_or_init(|| unsafe { mach_host_self() });
        let (mut cpus, mut info, mut count): (libc::natural_t, libc::processor_info_array_t, _) =
            (0, std::ptr::null_mut(), 0);
        // SAFETY: the flavor's array is `count` integers, which the kernel maps into this task and the call
        // points `info` at; it's released below.
        let result = unsafe {
            libc::host_processor_info(
                host,
                libc::PROCESSOR_CPU_LOAD_INFO,
                &raw mut cpus,
                &raw mut info,
                &raw mut count,
            )
        };
        if result != KERN_SUCCESS || info.is_null() {
            return None;
        }
        // SAFETY: the array holds one `processor_cpu_load_info` for each of `cpus` CPUs, as the flavor says.
        let loads = unsafe {
            std::slice::from_raw_parts(info.cast::<libc::processor_cpu_load_info>(), cpus as usize)
        };
        // Apple numbers a chip's efficiency cores first (`cores`).
        let efficiency = core_layout().map_or(0, |layout| layout.efficiency as usize);
        let mut ticks = CoreTicks::default();
        for (cpu, load) in loads.iter().enumerate() {
            let tick = |state: c_int| u64::from(load.cpu_ticks[state as usize]);
            let busy = tick(libc::CPU_STATE_USER) + tick(libc::CPU_STATE_SYSTEM) + tick(libc::CPU_STATE_NICE);
            let kind = if cpu < efficiency { &mut ticks.efficiency } else { &mut ticks.performance };
            kind.busy += busy;
            kind.all += busy + tick(libc::CPU_STATE_IDLE);
        }
        let bytes = count as usize * size_of::<libc::integer_t>();
        // SAFETY: the region the call mapped into this task, `count` integers long, which nothing reads after.
        unsafe { libc::vm_deallocate(mach_task_self(), info as libc::vm_address_t, bytes) };
        Some(ticks)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use std::time::Duration;

    use super::{CoreTicks, ProcessReads};

    pub(super) fn thread_cpu_time() -> Option<Duration> {
        super::unix_thread_cpu_time()
    }

    pub(super) fn process_reads() -> Option<ProcessReads> {
        let io = std::fs::read_to_string("/proc/self/io").ok()?;
        let line = io.lines().find_map(|line| line.strip_prefix("read_bytes:"))?;
        let disk_bytes = line.trim().parse().ok()?;
        // SAFETY: a struct of plain integers, which all-zero is a value of.
        let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
        // SAFETY: `usage` is the struct the call fills, for this process.
        if unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) } != 0 {
            return None;
        }
        Some(ProcessReads { disk_bytes, page_ins: u64::try_from(usage.ru_majflt).ok()? })
    }

    pub(super) fn core_ticks() -> Option<CoreTicks> {
        None
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "linux", target_os = "android")))]
mod os {
    use std::time::Duration;

    use super::{CoreTicks, ProcessReads};

    pub(super) fn thread_cpu_time() -> Option<Duration> {
        None
    }

    pub(super) fn process_reads() -> Option<ProcessReads> {
        None
    }

    pub(super) fn core_ticks() -> Option<CoreTicks> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Work on the thread moves its own clock, by no more than the wall time it took.
    #[test]
    fn a_threads_cpu_time_moves_with_its_work() {
        let Some(before) = thread_cpu_time() else { return };
        #[expect(
            clippy::disallowed_methods,
            reason = "the wall time the thread's clock is held to, in a test"
        )]
        let wall = std::time::Instant::now();
        let mut sum = 0_u64;
        for i in 0..20_000_000_u64 {
            sum = sum.wrapping_add(std::hint::black_box(i));
        }
        std::hint::black_box(sum);
        let (cpu, wall) = (thread_cpu_time().expect("read once").saturating_sub(before), wall.elapsed());
        assert!(cpu > Duration::ZERO, "{cpu:?}");
        // A clock's ticks may round a little past the wall clock's.
        assert!(cpu <= wall + Duration::from_millis(5), "{cpu:?} of {wall:?}");
    }

    /// A sleep is off the core: it barely moves the thread's clock.
    #[test]
    fn a_sleep_is_off_the_core() {
        let Some(before) = thread_cpu_time() else { return };
        std::thread::sleep(Duration::from_millis(30));
        let cpu = thread_cpu_time().expect("read once").saturating_sub(before);
        assert!(cpu < Duration::from_millis(10), "{cpu:?}");
    }

    /// Reads and ticks only add up, and a difference is what lies between two reads.
    #[test]
    fn counters_only_add_up() {
        if let Some(before) = process_reads() {
            let after = process_reads().expect("read once");
            assert!(after.disk_bytes >= before.disk_bytes && after.page_ins >= before.page_ins);
            assert_eq!(before.since(after), ProcessReads::default(), "never below zero");
        }
        if let Some(before) = core_ticks() {
            std::thread::sleep(Duration::from_millis(30));
            let since = core_ticks().expect("read once").since(before);
            let kinds = [since.performance, since.efficiency];
            assert!(kinds.iter().all(|ticks| ticks.busy <= ticks.all), "{since:?}");
            assert!(since.performance.all > 0, "30 ms is ticks on every core: {since:?}");
        }
        assert_eq!(Ticks { busy: 3, all: 12 }.busy_share(), Some(0.25));
        assert_eq!(Ticks::default().busy_share(), None);
    }
}
