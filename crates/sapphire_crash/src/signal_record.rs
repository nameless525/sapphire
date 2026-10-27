//! Android's native crash record, written from inside a signal handler.
//!
//! Nothing here allocates or locks: `core::fmt` writes into a fixed buffer on the stack, and the
//! file is written with async-signal-safe system calls only. The module is also compiled for
//! host tests (the writer on Unix hosts), so the format and the failure handling are checked
//! off-device.

use std::fmt::{self, Write as _};

/// A fixed-capacity text buffer. Output past the capacity is dropped, and the buffer remembers
/// that it was.
pub(crate) struct FixedBuf<const N: usize> {
    bytes: [u8; N],
    len: usize,
    truncated: bool,
}

impl<const N: usize> FixedBuf<N> {
    pub(crate) const fn new() -> Self {
        Self { bytes: [0; N], len: 0, truncated: false }
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Whether everything written to the buffer fitted.
    pub(crate) fn is_complete(&self) -> bool {
        !self.truncated
    }
}

impl<const N: usize> fmt::Write for FixedBuf<N> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(N - self.len);
        self.bytes[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        if n == s.len() {
            Ok(())
        } else {
            self.truncated = true;
            Err(fmt::Error)
        }
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

/// Appends the crash-time lines of a record. A full buffer truncates the output, which
/// [`FixedBuf::is_complete`] then reports.
pub(crate) fn write_crash_lines<const N: usize>(out: &mut FixedBuf<N>, info: &SignalInfo<'_>) {
    // The buffer records a truncation, so formatting errors need no handling here.
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

/// Writing the record file: async-signal-safe system calls only (open, flock, read, write, fsync,
/// close, rename), and every one's result is checked.
#[cfg(any(target_os = "android", all(test, unix)))]
pub(crate) mod file {
    use std::ffi::CStr;

    use super::FixedBuf;

    /// Writes a record to `partial` (the header, the crash lines, then a copy of `maps`) and
    /// renames it to `final_path` only once all of it is written and synced. Returns whether it
    /// was published.
    ///
    /// A record that couldn't be written whole, on a full disk, after an I/O error, or with crash
    /// lines that didn't fit their buffer, keeps its `.partial` name: `pending_reports` never
    /// lists it, so nothing mistakes it for a complete record, and a later launch's cleanup
    /// removes it once it is stale. The partial is locked as `report::create_partial` locks its
    /// files, so that cleanup leaves a live writer's alone.
    pub(crate) fn write_record<const N: usize>(
        partial: &CStr,
        final_path: &CStr,
        header: &[u8],
        crash_lines: &FixedBuf<N>,
        maps: &CStr,
    ) -> bool {
        let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC;
        // SAFETY: `partial` is a valid NUL-terminated path; open(2) is async-signal-safe.
        let fd = unsafe { libc::open(partial.as_ptr(), flags, 0o644) };
        if fd < 0 {
            return false;
        }
        // flock(2) is a plain system call, safe in a signal handler. Without a lock (a filesystem
        // that has none) the cleanup can't prove the writer gone, and keeps the file.
        // SAFETY: `fd` is open; flock only acts on it.
        unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

        // Every part is attempted, so a held-back partial keeps as much as could be written.
        let mut complete = crash_lines.is_complete();
        complete &= write_all(fd, header);
        complete &= write_all(fd, crash_lines.as_bytes());
        complete &= write_all(fd, b"\nmaps:\n");
        complete &= copy_file(maps, fd);
        complete &= sync_and_close(fd);
        // SAFETY: both paths are valid NUL-terminated strings; rename(2) is async-signal-safe.
        complete && unsafe { libc::rename(partial.as_ptr(), final_path.as_ptr()) } == 0
    }

    /// Writes all of `bytes`, retrying after signal interruptions. `false` on any other failure,
    /// such as a full disk, or a write that makes no progress.
    pub(crate) fn write_all(fd: libc::c_int, mut bytes: &[u8]) -> bool {
        while !bytes.is_empty() {
            // SAFETY: `bytes` is valid for reads of `bytes.len()` bytes; write(2) is
            // async-signal-safe.
            let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
            if written > 0 {
                bytes = &bytes[written.unsigned_abs().min(bytes.len())..];
            } else if written == 0 || errno() != libc::EINTR {
                return false;
            }
        }
        true
    }

    /// Appends the whole of the file at `path` to `out`. `false` if it can't be opened or read to
    /// its end, or a write fails.
    pub(crate) fn copy_file(path: &CStr, out: libc::c_int) -> bool {
        // SAFETY: `path` is a valid NUL-terminated path; open(2) is async-signal-safe.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            return false;
        }
        let mut buf = [0u8; 1024];
        let copied = loop {
            // SAFETY: `buf` is valid for writes of `buf.len()` bytes; read(2) is async-signal-safe.
            let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if read > 0 {
                if !write_all(out, &buf[..read.unsigned_abs().min(buf.len())]) {
                    break false;
                }
            } else if read == 0 {
                break true;
            } else if errno() != libc::EINTR {
                break false;
            }
        };
        // SAFETY: `fd` was opened above and is not used again.
        unsafe { libc::close(fd) };
        copied
    }

    /// Flushes `fd` to storage and closes it. `false` if the data may not have reached the disk.
    pub(crate) fn sync_and_close(fd: libc::c_int) -> bool {
        // SAFETY: `fd` is open and owned by the caller, which doesn't use it again; fsync(2) and
        // close(2) are async-signal-safe.
        unsafe {
            let synced = libc::fsync(fd) == 0;
            libc::close(fd);
            synced
        }
    }

    fn errno() -> libc::c_int {
        // SAFETY: each returns a valid pointer to the calling thread's errno.
        unsafe {
            #[cfg(target_os = "android")]
            let errno = libc::__errno();
            #[cfg(target_os = "linux")]
            let errno = libc::__errno_location();
            #[cfg(target_vendor = "apple")]
            let errno = libc::__error();
            *errno
        }
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
        assert!(out.is_complete());
    }

    #[test]
    fn truncates_instead_of_overflowing() {
        let mut out = FixedBuf::<8>::new();
        assert!(out.write_str("12345").is_ok());
        assert!(out.is_complete());
        assert!(out.write_str("67890").is_err());
        assert_eq!(out.as_bytes(), b"12345678");
        assert!(!out.is_complete());
        assert!(out.write_str("x").is_err());
        assert_eq!(out.as_bytes().len(), 8);
    }

    #[test]
    fn the_largest_crash_lines_fit_the_handlers_buffer() {
        // The handler formats into a `FixedBuf<1024>`; the worst case must fit, or every record
        // would be held back as incomplete.
        let mut out = FixedBuf::<1024>::new();
        let registers = [("pc", u64::MAX), ("sp", u64::MAX), ("fp", u64::MAX), ("lr", u64::MAX)];
        let info = SignalInfo {
            signal: u32::MAX,
            code: i32::MIN,
            fault_address: u64::MAX,
            pid: i32::MIN,
            tid: i32::MIN,
            registers: &registers,
        };
        write_crash_lines(&mut out, &info);
        assert!(out.is_complete(), "{} bytes", out.as_bytes().len());
    }

    #[test]
    fn header_keeps_one_value_per_line() {
        assert_eq!(
            header("game\nevil: 1", "1.0", "1-2"),
            "Sapphire native crash record v1\napp: game evil: 1\nrelease: 1.0\nrun: 1-2\n"
        );
    }

    #[cfg(unix)]
    mod file_tests {
        use std::ffi::CString;
        use std::os::fd::IntoRawFd;
        use std::path::Path;

        use super::super::file::{copy_file, sync_and_close, write_all, write_record};
        use super::super::*;
        use crate::test_support::ScratchDir;

        fn c_path(path: &Path) -> CString {
            CString::new(path.as_os_str().as_encoded_bytes()).expect("no NUL")
        }

        fn crash_lines() -> FixedBuf<1024> {
            let mut lines = FixedBuf::<1024>::new();
            let registers = [("pc", 0x7a_1234_5678)];
            write_crash_lines(
                &mut lines,
                &SignalInfo {
                    signal: 11,
                    code: 1,
                    fault_address: 0,
                    pid: 100,
                    tid: 101,
                    registers: &registers,
                },
            );
            lines
        }

        /// A pipe's two ends, as raw descriptors the caller owns.
        fn pipe() -> (libc::c_int, libc::c_int) {
            let mut fds = [0; 2];
            // SAFETY: `fds` has room for the two descriptors pipe(2) returns.
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
            (fds[0], fds[1])
        }

        fn close(fd: libc::c_int) {
            // SAFETY: the test owns `fd` and doesn't use it again.
            unsafe { libc::close(fd) };
        }

        #[test]
        fn publishes_a_complete_record() {
            let dir = ScratchDir::new("record-complete");
            let maps = dir.path().join("maps");
            std::fs::write(
                &maps,
                "7a00000000-7a00100000 r-xp 00000000 fd:00 1 /data/app/libgame.so\n".repeat(40),
            )
            .expect("write maps");
            let partial = dir.path().join("1-2.native.txt.partial");
            let complete = dir.path().join("1-2.native.txt");

            let lines = crash_lines();
            assert!(write_record(&c_path(&partial), &c_path(&complete), b"head\n", &lines, &c_path(&maps)));

            let text = std::fs::read(&complete).expect("the published record");
            let mut expected = b"head\n".to_vec();
            expected.extend_from_slice(lines.as_bytes());
            expected.extend_from_slice(b"\nmaps:\n");
            expected.extend_from_slice(&std::fs::read(&maps).expect("maps"));
            assert_eq!(text, expected);
            assert!(!partial.exists());
        }

        #[test]
        fn holds_back_a_record_whose_maps_are_missing() {
            // PA-006: before, the record was renamed into place whatever had failed, so a
            // truncated file looked like a complete `.native.txt`.
            let dir = ScratchDir::new("record-no-maps");
            let partial = dir.path().join("1-2.native.txt.partial");
            let complete = dir.path().join("1-2.native.txt");
            let missing = dir.path().join("no-such-maps");

            assert!(!write_record(
                &c_path(&partial),
                &c_path(&complete),
                b"head\n",
                &crash_lines(),
                &c_path(&missing)
            ));
            assert!(!complete.exists(), "an incomplete record must not be published");
            assert!(partial.exists(), "it stays behind as a partial, for the cleanup");
            assert!(crate::report::pending_reports(dir.path()).expect("list").is_empty());
        }

        /// The record's own writes fail: `/dev/full` takes the partial's place.
        #[cfg(target_os = "linux")]
        #[test]
        fn holds_back_a_record_on_a_full_disk() {
            let dir = ScratchDir::new("record-full");
            let maps = dir.path().join("maps");
            std::fs::write(&maps, "maps\n").expect("write maps");
            let complete = dir.path().join("1-2.native.txt");

            assert!(!write_record(
                c"/dev/full",
                &c_path(&complete),
                b"head\n",
                &crash_lines(),
                &c_path(&maps)
            ));
            assert!(!complete.exists());
        }

        #[test]
        fn holds_back_a_record_whose_crash_lines_were_truncated() {
            let dir = ScratchDir::new("record-truncated");
            let maps = dir.path().join("maps");
            std::fs::write(&maps, "maps\n").expect("write maps");
            let partial = dir.path().join("1-2.native.txt.partial");
            let complete = dir.path().join("1-2.native.txt");
            let mut lines = FixedBuf::<8>::new();
            let _ = lines.write_str("signal: 11 (SIGSEGV)\n");

            assert!(!write_record(&c_path(&partial), &c_path(&complete), b"head\n", &lines, &c_path(&maps)));
            assert!(!complete.exists());
        }

        #[test]
        fn holds_back_a_record_it_cannot_open() {
            let dir = ScratchDir::new("record-no-dir");
            let partial = dir.path().join("missing-dir").join("1-2.native.txt.partial");
            let complete = dir.path().join("1-2.native.txt");
            let maps = dir.path().join("maps");
            std::fs::write(&maps, "maps\n").expect("write maps");

            assert!(!write_record(
                &c_path(&partial),
                &c_path(&complete),
                b"head\n",
                &crash_lines(),
                &c_path(&maps)
            ));
            assert!(!complete.exists());
        }

        #[test]
        fn write_failures_are_reported() {
            // A descriptor that isn't open (EBADF).
            let (read_end, write_end) = pipe();
            close(read_end);
            close(write_end);
            assert!(!write_all(write_end, b"x"));

            // A pipe nobody reads any more (EPIPE; Rust programs ignore SIGPIPE).
            let (read_end, write_end) = pipe();
            close(read_end);
            assert!(!write_all(write_end, b"x"));
            close(write_end);

            // A full disk (ENOSPC).
            #[cfg(target_os = "linux")]
            {
                let full = std::fs::OpenOptions::new().write(true).open("/dev/full").expect("/dev/full");
                let fd = full.into_raw_fd();
                assert!(!write_all(fd, b"x"));
                close(fd);
            }

            // Nothing to write always succeeds.
            let (read_end, write_end) = pipe();
            assert!(write_all(write_end, b""));
            assert!(write_all(write_end, b"x"));
            close(read_end);
            close(write_end);
        }

        #[test]
        fn read_failures_are_reported() {
            let dir = ScratchDir::new("record-read");
            let (read_end, write_end) = pipe();
            // A directory opens for reading, but reading it fails (EISDIR).
            assert!(!copy_file(&c_path(dir.path()), write_end));
            assert!(!copy_file(&c_path(&dir.path().join("missing")), write_end));
            close(read_end);
            close(write_end);
        }

        #[test]
        fn sync_failures_are_reported() {
            // fsync(2) on a pipe fails (EINVAL), as it can on a failing disk.
            let (read_end, write_end) = pipe();
            assert!(!sync_and_close(write_end));
            close(read_end);

            let dir = ScratchDir::new("record-sync");
            let file = std::fs::File::create(dir.path().join("x")).expect("create");
            assert!(sync_and_close(file.into_raw_fd()));
        }
    }
}
