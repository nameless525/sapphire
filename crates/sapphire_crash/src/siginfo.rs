//! The address a Linux or Android signal faulted at, which `crash-handler` 0.8.1 loses.
//!
//! Its signal handler copies the kernel's `siginfo_t` byte for byte into the crash context's
//! `signalfd_siginfo`, which lays the same 128 bytes out differently: `si_signo`, `si_errno` and
//! `si_code` line up, but `si_addr` (byte 16) lands in `ssi_uid` and `ssi_fd`, and `ssi_addr`
//! (byte 72) reads past it, 0 for a fault. So Linux minidumps and Android signal records said every
//! crash was at 0 (M4 C2's As built note). Its `simulate_signal`, which panic dumps use, fills a
//! real `signalfd_siginfo`, with `SI_USER` as its code; this reads the address only where the
//! kernel raised a fault, so that one is left as it is.

/// Both `siginfo_t` and `signalfd_siginfo` are 128 bytes on Linux.
pub(crate) const SIGINFO_SIZE: usize = 128;

/// Signals the kernel gives a faulting address (`si_addr`), by Linux's numbers: SIGILL, SIGTRAP,
/// SIGBUS, SIGFPE and SIGSEGV.
const FAULT_SIGNALS: [u32; 5] = [4, 5, 7, 8, 11];

/// The address `siginfo`, a kernel `siginfo_t` as `crash-handler` copied it, says the fault was
/// at; `None` for a signal that has none or that something other than a fault raised (a code of 0
/// or less: `kill`, `tgkill`, `sigqueue`).
pub(crate) fn fault_address(siginfo: &[u8; SIGINFO_SIZE]) -> Option<u64> {
    let word =
        |at: usize| u32::from_ne_bytes([siginfo[at], siginfo[at + 1], siginfo[at + 2], siginfo[at + 3]]);
    let (signal, code) = (word(0), word(8).cast_signed());
    let mut address = [0; 8];
    address.copy_from_slice(&siginfo[16..24]);
    (code > 0 && FAULT_SIGNALS.contains(&signal)).then(|| u64::from_ne_bytes(address))
}

/// The fault address of the crash `context` describes, as [`fault_address`] reads it.
/// Async-signal-safe.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) fn fault_address_of(context: &crash_handler::CrashContext) -> Option<u64> {
    const { assert!(size_of::<libc::signalfd_siginfo>() == SIGINFO_SIZE) };
    // SAFETY: `signalfd_siginfo` is 128 bytes of plain integers (asserted above), so its bytes can be
    // read as an array of them for as long as `context` is borrowed.
    let bytes = unsafe { &*std::ptr::from_ref(&context.siginfo).cast::<[u8; SIGINFO_SIZE]>() };
    fault_address(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `siginfo_t` as the kernel fills one: the signal, its code, and `si_addr` at byte 16.
    fn siginfo(signal: u32, code: i32, address: u64) -> [u8; SIGINFO_SIZE] {
        let mut bytes = [0; SIGINFO_SIZE];
        bytes[0..4].copy_from_slice(&signal.to_ne_bytes());
        bytes[8..12].copy_from_slice(&code.to_ne_bytes());
        bytes[16..24].copy_from_slice(&address.to_ne_bytes());
        bytes
    }

    #[test]
    fn a_fault_reads_its_address_where_the_kernel_put_it() {
        // SIGSEGV, SEGV_MAPERR, at `forced_fault`'s address.
        let fault = siginfo(11, 1, 0x10);
        assert_eq!(fault_address(&fault), Some(0x10));
        // Where `signalfd_siginfo` keeps `ssi_addr`, a fault's `siginfo_t` has nothing.
        assert_eq!(fault[72..80], [0; 8]);
        assert_eq!(fault_address(&siginfo(7, 2, 0x7f00_dead_0000)), Some(0x7f00_dead_0000));
    }

    #[test]
    fn signals_without_a_fault_have_no_address() {
        // An abort (`tgkill`, SI_TKILL), a SIGSEGV someone sent with `kill` (SI_USER), and the
        // `signalfd_siginfo` `simulate_signal` makes for a panic dump, whose byte 16 is `ssi_uid`.
        assert_eq!(fault_address(&siginfo(6, -6, 0)), None);
        assert_eq!(fault_address(&siginfo(11, 0, 501)), None);
        assert_eq!(fault_address(&siginfo(6, 0, 501)), None);
    }
}
