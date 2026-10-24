//! Formatting for Android's native crash record, usable inside a signal handler.
//!
//! Nothing here allocates or locks: `core::fmt` writes into a fixed buffer on the stack. The
//! module is also compiled for host tests, so the format is checked off-device.

use std::fmt::{self, Write as _};

/// A fixed-capacity text buffer. Output past the capacity is dropped.
pub(crate) struct FixedBuf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> FixedBuf<N> {
    pub(crate) const fn new() -> Self {
        Self { bytes: [0; N], len: 0 }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

impl<const N: usize> fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(N - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        if n == s.len() { Ok(()) } else { Err(fmt::Error) }
    }
}

/// What the signal handler knows about the crash.
pub(crate) struct SignalInfo<'a> {
    pub signal: u32,
    pub code: i32,
    pub fault_address: u64,
    pub pid: i32,
    pub tid: i32,
    /// Named registers of the crashing thread, such as `pc` and `lr`.
    pub registers: &'a [(&'static str, u64)],
}

/// The first lines of a record, prepared at install time when allocating is still fine.
pub(crate) fn header(app_name: &str, release: &str, run_id: &str) -> String {
    let one_line = |s: &str| s.replace(['\r', '\n'], " ");
    format!(
        "Sapphire native crash record v1\napp: {}\nrelease: {}\nrun: {}\n",
        one_line(app_name),
        one_line(release),
        one_line(run_id),
    )
}

/// Appends the crash-time lines of a record. A full buffer truncates the output; it never fails.
pub(crate) fn write_crash_lines<const N: usize>(out: &mut FixedBuf<N>, info: &SignalInfo<'_>) {
    // A truncated record is still worth writing, so formatting errors are ignored.
    let _ = write!(
        out,
        "signal: {} ({})\ncode: {}\nfault_address: {:#018x}\npid: {}\ntid: {}\n",
        info.signal,
        signal_name(info.signal),
        info.code,
        info.fault_address,
        info.pid,
        info.tid,
    );
    for (name, value) in info.registers {
        let _ = writeln!(out, "{name}: {value:#018x}");
    }
}

/// Names of the signals `crash-handler` catches, by their Linux numbers (macOS numbers differ,
/// which is why `libc` constants are not used: this module is also built for host tests).
fn signal_name(signal: u32) -> &'static str {
    match signal {
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        7 => "SIGBUS",
        8 => "SIGFPE",
        11 => "SIGSEGV",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_record() {
        let mut out = FixedBuf::<512>::new();
        let registers = [("pc", 0x7a_1234_5678), ("lr", 0x7a_1234_0000)];
        write_crash_lines(
            &mut out,
            &SignalInfo { signal: 11, code: 1, fault_address: 0, pid: 100, tid: 101, registers: &registers },
        );
        let text = std::str::from_utf8(out.as_bytes()).expect("utf-8");
        assert_eq!(
            text,
            "signal: 11 (SIGSEGV)\ncode: 1\nfault_address: 0x0000000000000000\npid: 100\ntid: 101\n\
             pc: 0x0000007a12345678\nlr: 0x0000007a12340000\n"
        );
    }

    #[test]
    fn truncates_instead_of_overflowing() {
        let mut out = FixedBuf::<8>::new();
        assert!(out.write_str("12345").is_ok());
        assert!(out.write_str("67890").is_err());
        assert_eq!(out.as_bytes(), b"12345678");
        assert!(out.write_str("x").is_err());
        assert_eq!(out.as_bytes().len(), 8);
    }

    #[test]
    fn header_keeps_one_value_per_line() {
        assert_eq!(
            header("game\nevil: 1", "1.0", "1-2"),
            "Sapphire native crash record v1\napp: game evil: 1\nrelease: 1.0\nrun: 1-2\n"
        );
    }
}
