//! Android native crash capture: an in-process signal record (spec §5.2, open question Q12).
//!
//! `minidumper` needs a monitor process that an Android app cannot launch, and `minidump-writer`
//! cannot dump the process it runs in (see the crate docs). So the crash handler writes a short
//! text record from inside the signal handler: signal, fault address, the crashing thread's key
//! registers and a copy of `/proc/self/maps`, which together locate the faulting instruction in
//! its `.so` for `addr2line`. It then declines the signal so the previous handler, normally
//! `debuggerd`, still writes its tombstone and logcat backtrace.

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crash_handler::{CrashContext, CrashEventResult, CrashHandler};

use crate::CrashConfig;
use crate::report;
use crate::signal_record::{self, FixedBuf, SignalInfo};

/// Why the signal handler could not be installed. Only ever logged.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SignalCaptureError {
    #[error("the dump path {0} contains a NUL byte")]
    NulInPath(String),
    #[error("cannot attach the crash handler: {0}")]
    Handler(crash_handler::Error),
}

/// Keeps the signal handler attached; dropping it restores the previous handlers.
pub(crate) struct SignalCapture {
    _handler: CrashHandler,
}

impl SignalCapture {
    pub(crate) fn start(config: &CrashConfig, run_id: &str) -> Result<Self, SignalCaptureError> {
        // Everything the handler needs is prepared now, while allocating is still allowed.
        let final_name = report::native_file_name(run_id);
        let partial_path = c_path(&config.dump_dir.join(report::partial_name(&final_name)))?;
        let final_path = c_path(&config.dump_dir.join(&final_name))?;
        let header = signal_record::header(&config.app_name, &config.release, run_id).into_bytes();

        // SAFETY: the event runs inside a signal handler of a crashed process. `write_record`
        // only uses async-signal-safe system calls (open, read, write, fsync, close, rename) and
        // formats into a stack buffer; it never allocates or locks.
        let event = unsafe {
            crash_handler::make_crash_event(move |context| {
                write_record(&partial_path, &final_path, &header, context);
                // Not handled: crash-handler restores the previous handler and re-raises.
                CrashEventResult::Handled(false)
            })
        };
        let handler = CrashHandler::attach(event).map_err(SignalCaptureError::Handler)?;
        Ok(Self { _handler: handler })
    }
}

fn c_path(path: &Path) -> Result<CString, SignalCaptureError> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| SignalCaptureError::NulInPath(path.display().to_string()))
}

/// Writes the record to `partial` and renames it to `final_path`. Async-signal-safe.
fn write_record(partial: &CStr, final_path: &CStr, header: &[u8], context: &CrashContext) {
    let flags = libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC | libc::O_CLOEXEC;
    // SAFETY: `partial` is a valid NUL-terminated path; open(2) is async-signal-safe.
    let fd = unsafe { libc::open(partial.as_ptr(), flags, 0o644) };
    if fd < 0 {
        return;
    }
    // The writer's lock, as `report::create_partial` takes it: a later launch's cleanup removes
    // the partial only once it is free. flock(2) is a plain system call, safe in a signal handler.
    // SAFETY: `fd` is open; flock only acts on it.
    unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };

    write_all(fd, header);
    let registers = registers(context);
    let info = SignalInfo {
        signal: context.siginfo.ssi_signo,
        code: context.siginfo.ssi_code,
        fault_address: context.siginfo.ssi_addr,
        pid: context.pid,
        tid: context.tid,
        registers: &registers,
    };
    let mut lines = FixedBuf::<1024>::new();
    signal_record::write_crash_lines(&mut lines, &info);
    write_all(fd, lines.as_bytes());
    write_all(fd, b"\nmaps:\n");
    copy_file(c"/proc/self/maps", fd);

    // SAFETY: `fd` is open and owned here; fsync, close and rename are async-signal-safe, and
    // both paths are valid NUL-terminated strings.
    unsafe {
        libc::fsync(fd);
        libc::close(fd);
        libc::rename(partial.as_ptr(), final_path.as_ptr());
    }
}

/// The crashing thread's program counter, stack pointer, frame pointer and return address.
#[cfg(target_arch = "aarch64")]
fn registers(context: &CrashContext) -> [(&'static str, u64); 4] {
    let mcontext = &context.context.uc_mcontext;
    [("pc", mcontext.pc), ("sp", mcontext.sp), ("fp", mcontext.regs[29]), ("lr", mcontext.regs[30])]
}

/// The crashing thread's instruction, stack and frame pointers (`REG_RIP`, `REG_RSP`, `REG_RBP`).
#[cfg(target_arch = "x86_64")]
fn registers(context: &CrashContext) -> [(&'static str, u64); 3] {
    let gregs = &context.context.uc_mcontext.gregs;
    [
        ("rip", gregs[16].cast_unsigned()),
        ("rsp", gregs[15].cast_unsigned()),
        ("rbp", gregs[10].cast_unsigned()),
    ]
}

/// Spec §2 ships arm64 Android, plus x86_64 for the emulator; other ABIs record no registers.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
fn registers(_context: &CrashContext) -> [(&'static str, u64); 0] {
    []
}

fn write_all(fd: libc::c_int, mut bytes: &[u8]) {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is valid for reads of `bytes.len()` bytes; write(2) is
        // async-signal-safe.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written.unsigned_abs().min(bytes.len())..];
        } else if written == 0 || errno() != libc::EINTR {
            return;
        }
    }
}

fn copy_file(path: &CStr, out: libc::c_int) {
    // SAFETY: `path` is a valid NUL-terminated path; open(2) is async-signal-safe.
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return;
    }
    let mut buf = [0u8; 1024];
    loop {
        // SAFETY: `buf` is valid for writes of `buf.len()` bytes; read(2) is async-signal-safe.
        let read = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read > 0 {
            write_all(out, &buf[..read.unsigned_abs().min(buf.len())]);
        } else if read == 0 || errno() != libc::EINTR {
            break;
        }
    }
    // SAFETY: `fd` was opened above and is not used again.
    unsafe { libc::close(fd) };
}

fn errno() -> libc::c_int {
    // SAFETY: `__errno` returns a valid pointer to the calling thread's errno.
    unsafe { *libc::__errno() }
}
