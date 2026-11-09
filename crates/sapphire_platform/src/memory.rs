//! What the process holds in memory, as its OS counts it (M3 Z1): what a bench reads beside the engine's own
//! accounting across a zone's walk, and the one place that knows how each OS says it.
//!
//! | OS | Now | Most |
//! |---|---|---|
//! | macOS | `proc_pid_rusage`'s physical footprint, what `/usr/bin/footprint` and Activity Monitor show | its lifetime most |
//! | Linux, Android | `/proc/self/status`' `VmRSS`, resident pages | `VmHWM` |
//! | iOS, Windows | not read yet | |
//!
//! The two kinds don't compare: a footprint counts compressed and GPU-tagged pages a resident set doesn't.

/// The process's memory, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessMemory {
    pub now: u64,
    /// The most it's held since it started.
    pub most: u64,
}

/// The process's memory as its OS counts it: the footprint on macOS, resident memory on Linux and
/// Android. `None` where it isn't read or the OS doesn't say. On macOS it allocates nothing; on Linux
/// and Android it reads a file.
#[must_use]
pub fn process_memory() -> Option<ProcessMemory> {
    os::read()
}

#[cfg(target_os = "macos")]
mod os {
    use super::ProcessMemory;

    pub(super) fn read() -> Option<ProcessMemory> {
        let mut info = std::mem::MaybeUninit::<libc::rusage_info_v4>::zeroed();
        // SAFETY: `info` is as large as the flavor asked for, and the call writes no more than that; it reads
        // this process, which is alive.
        let result =
            unsafe { libc::proc_pid_rusage(libc::getpid(), libc::RUSAGE_INFO_V4, info.as_mut_ptr().cast()) };
        if result != 0 {
            return None;
        }
        // SAFETY: the call succeeded, so it filled `info`; zeroed, it was a valid value already.
        let info = unsafe { info.assume_init() };
        Some(ProcessMemory { now: info.ri_phys_footprint, most: info.ri_lifetime_max_phys_footprint })
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
        Some(ProcessMemory { now: kib("VmRSS:")?, most: kib("VmHWM:")? })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
mod os {
    use super::ProcessMemory;

    pub(super) fn read() -> Option<ProcessMemory> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where it's read, it's this process's: some memory, and never more now than at its most.
    #[test]
    fn the_process_holds_what_it_says() {
        let read_here = cfg!(any(target_os = "macos", target_os = "linux", target_os = "android"));
        let Some(memory) = process_memory() else {
            assert!(!read_here, "it's read on this OS");
            return;
        };
        assert!(memory.now > 1 << 20, "{memory:?}");
        assert!(memory.most >= memory.now, "{memory:?}");
    }
}
