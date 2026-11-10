//! What the process holds in memory, as its OS counts it (M3 Z1, Z5b): what a bench reads beside the engine's
//! own accounting across a zone's walk, and the one place that knows how each OS says it.
//!
//! | OS | Now | Most |
//! |---|---|---|
//! | macOS, iOS | `task_info`'s `TASK_VM_INFO` physical footprint, what `/usr/bin/footprint`, Activity Monitor and Xcode's memory gauge show, and what iOS kills an app by | its lifetime most (`ledger_phys_footprint_peak`) |
//! | Linux, Android | `/proc/self/status`' `VmRSS`, resident pages | `VmHWM` |
//! | Windows | `GetProcessMemoryInfo`'s working set, resident pages | its peak working set |
//!
//! The two kinds don't compare: a footprint counts compressed and GPU-tagged pages a resident set doesn't.

/// The process's memory, in bytes. `most` is never less than `now`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub now: u64,
    /// The most it's held since it started.
    pub most: u64,
}

impl ProcessMemory {
    /// An OS's two counters, read in the same call, with the most held to at least now. Apple's kernel moves
    /// the footprint's lifetime most behind the footprint itself, so a read can find the footprint a page
    /// over it: one macOS read gave a footprint of 3,473,936 bytes and a most of 3,457,552, 16 KiB, one page,
    /// apart. What it has held now, it has held at its most.
    fn of(now: u64, most: u64) -> Self {
        Self { now, most: most.max(now) }
    }
}

/// The process's memory as its OS counts it: the footprint on macOS and iOS, resident memory on Linux,
/// Android and Windows. `None` where the OS doesn't say. On Apple's systems and Windows it allocates
/// nothing; on Linux and Android it reads a file.
#[must_use]
pub fn process_memory() -> Option<ProcessMemory> {
    os::read()
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod os {
    use std::mem::{offset_of, size_of};

    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{TASK_VM_INFO, task_vm_info};
    use mach2::traps::mach_task_self;
    use mach2::vm_types::natural_t;

    use super::ProcessMemory;

    /// The struct's size in the `natural_t`s `task_info` counts in.
    const FULL: u32 = (size_of::<task_vm_info>() / size_of::<natural_t>()) as u32;
    /// How much of it a kernel must fill for the peak to be there: the revision that added
    /// `ledger_phys_footprint_peak`, which every macOS and iOS the engine supports has.
    const WITH_PEAK: u32 = ((offset_of!(task_vm_info, ledger_phys_footprint_peak) + size_of::<i64>())
        / size_of::<natural_t>()) as u32;

    pub(super) fn read() -> Option<ProcessMemory> {
        let mut info = task_vm_info::default();
        let mut count = FULL;
        // SAFETY: `info` is `count` naturals long, and the call writes no more than that and says how many it
        // wrote; `mach_task_self` names this task, which is alive, and needs no release.
        let result =
            unsafe { task_info(mach_task_self(), TASK_VM_INFO, (&raw mut info).cast(), &raw mut count) };
        if result != KERN_SUCCESS || count < WITH_PEAK {
            return None;
        }
        let most = u64::try_from(info.ledger_phys_footprint_peak).ok()?;
        Some(ProcessMemory::of(info.phys_footprint, most))
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use super::ProcessMemory;

    pub(super) fn read() -> Option<ProcessMemory> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let kib = |field: &str| -> Option<u64> {
            let line = status.lines().find(|line| line.starts_with(field))?;
            line[field.len()..].trim().trim_end_matches("kB").trim().parse::<u64>().ok().map(|kib| kib * 1024)
        };
        Some(ProcessMemory::of(kib("VmRSS:")?, kib("VmHWM:")?))
    }
}

#[cfg(target_os = "windows")]
mod os {
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    use super::ProcessMemory;

    pub(super) fn read() -> Option<ProcessMemory> {
        let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS>()).ok()?;
        let mut counters = PROCESS_MEMORY_COUNTERS { cb: size, ..PROCESS_MEMORY_COUNTERS::default() };
        // SAFETY: `counters` is `size` bytes, which the call writes no more than; `GetCurrentProcess`'s
        // pseudo-handle names this process, which is alive, and needs no closing.
        if unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &raw mut counters, size) } == 0 {
            return None;
        }
        let bytes = |count: usize| u64::try_from(count).ok();
        Some(ProcessMemory::of(bytes(counters.WorkingSetSize)?, bytes(counters.PeakWorkingSetSize)?))
    }
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android",
    target_os = "windows"
)))]
mod os {
    use super::ProcessMemory;

    pub(super) fn read() -> Option<ProcessMemory> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// It's read on every OS the engine ships on, and it's this process's: some memory, and never more now
    /// than at its most.
    #[test]
    fn the_process_holds_what_it_says() {
        let memory = process_memory().expect("it's read on this OS");
        assert!(memory.now > 1 << 20, "{memory:?}");
        assert!(memory.most >= memory.now, "{memory:?}");
    }

    /// The read that once found the footprint over its most (M3 Z5b): the most is held to now, and a most
    /// already over now is left as it is.
    #[test]
    fn the_most_is_never_less_than_now() {
        assert_eq!(
            ProcessMemory::of(3_473_936, 3_457_552),
            ProcessMemory { now: 3_473_936, most: 3_473_936 }
        );
        assert_eq!(
            ProcessMemory::of(3_457_552, 3_473_936),
            ProcessMemory { now: 3_457_552, most: 3_473_936 }
        );
    }
}
