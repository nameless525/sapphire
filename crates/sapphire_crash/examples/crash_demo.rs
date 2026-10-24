//! Crashes on purpose so crash capture can be checked end to end.
//!
//! ```text
//! cargo run -p sapphire_crash --example crash_demo -- <dump-dir> panic    # a panic report
//! cargo run -p sapphire_crash --example crash_demo -- <dump-dir> native   # a minidump (desktop)
//! cargo run -p sapphire_crash --example crash_demo -- <dump-dir> --list   # what the runs left
//! ```
//!
//! Add `--profile shipping` to see `panic = "abort"`: a panic then leaves its report and, on
//! macOS and Linux, a minidump of the abort that follows.

// A command-line demo: printing to stdout is its job.
#![allow(clippy::print_stdout)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sapphire_crash::{CrashConfig, CrashGuard};

fn main() -> ExitCode {
    // First line of main: when this process is the crash monitor, this never returns.
    sapphire_crash::run_monitor_if_requested();

    tracing_subscriber::fmt().init();

    let mut args = std::env::args_os().skip(1);
    let (Some(dir), Some(mode), None) = (args.next(), args.next(), args.next()) else {
        return usage();
    };
    let dir = PathBuf::from(dir);
    match mode.to_str() {
        Some("--list") => list(&dir),
        Some("panic") => {
            let Some(_guard) = install(&dir) else { return ExitCode::FAILURE };
            panic!("crash_demo: deliberate panic");
        }
        Some("native") => {
            let Some(_guard) = install(&dir) else { return ExitCode::FAILURE };
            crash_natively();
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: crash_demo <dump-dir> panic|native|--list");
    ExitCode::from(2)
}

fn install(dir: &Path) -> Option<CrashGuard> {
    let config = CrashConfig::new("crash_demo", env!("CARGO_PKG_VERSION"), dir);
    match sapphire_crash::install(config) {
        Ok(guard) => {
            println!("run {}: native capture {:?}", guard.run_id(), guard.native_capture());
            Some(guard)
        }
        Err(err) => {
            let cause = std::error::Error::source(&err).map(|cause| format!(": {cause}")).unwrap_or_default();
            eprintln!("crash_demo: cannot install crash capture: {err}{cause}");
            None
        }
    }
}

fn crash_natively() -> ! {
    println!("writing through a null pointer");
    // SAFETY: none, deliberately. This writes through a null pointer to raise a real access
    // violation (SIGSEGV, EXC_BAD_ACCESS), which is the point of the demo. The write is volatile
    // so the compiler keeps it.
    unsafe { std::ptr::null_mut::<u32>().write_volatile(0xDEAD) };
    // Unreachable in practice; abort rather than return if the write somehow succeeded.
    std::process::abort()
}

fn list(dir: &Path) -> ExitCode {
    let reports = match sapphire_crash::pending_reports(dir) {
        Ok(reports) => reports,
        Err(err) => {
            eprintln!("crash_demo: {err}");
            return ExitCode::FAILURE;
        }
    };
    println!("{} pending report(s) in {}", reports.len(), dir.display());
    for report in &reports {
        let size = std::fs::metadata(report.path()).map_or(0, |m| m.len());
        println!(
            "  {:<12} run {}  {:>9} bytes  {}",
            format!("{:?}", report.kind()),
            report.run_id(),
            size,
            report.path().display()
        );
    }
    ExitCode::SUCCESS
}
