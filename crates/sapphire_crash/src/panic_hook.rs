//! The process-wide panic hook, installed on every platform.

use std::backtrace::Backtrace;
use std::panic::{self, PanicHookInfo};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Instant;

use crate::report::{self, PanicReport};

/// What the hook stamps into every report.
pub(crate) struct HookContext {
    pub app_name: String,
    pub release: String,
    pub run_id: String,
    pub dump_dir: PathBuf,
}

struct HookState {
    context: HookContext,
    armed: AtomicBool,
    next_seq: AtomicU32,
    /// Set once a native minidump of a panic has been requested, so repeated or concurrent panics
    /// do not ask the (now exited) monitor again.
    dump_requested: AtomicBool,
    started: Instant,
}

/// Keeps the hook armed. Dropping it disarms the hook.
///
/// A hook cannot be taken out of the chain without also removing hooks installed after it, so a
/// disarmed hook stays in place and only forwards to the hook it replaced.
pub(crate) struct Registration {
    state: Arc<HookState>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.state.armed.store(false, Ordering::Release);
    }
}

/// Installs the hook in front of the current one.
pub(crate) fn install(context: HookContext) -> Registration {
    // Uptime in panic reports is diagnostic metadata, never simulation time (rule R2).
    #[allow(clippy::disallowed_methods)]
    let started = Instant::now();
    let state = Arc::new(HookState {
        context,
        armed: AtomicBool::new(true),
        next_seq: AtomicU32::new(0),
        dump_requested: AtomicBool::new(false),
        started,
    });

    let hook_state = Arc::clone(&state);
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        if hook_state.armed.load(Ordering::Acquire) {
            hook_state.report(info);
        }
        previous(info);
    }));
    Registration { state }
}

impl HookState {
    fn report(&self, info: &PanicHookInfo<'_>) {
        let report = self.describe(info);

        // Write the file before logging anything. Under `panic = "abort"` (the shipping profile)
        // the process aborts as soon as the hook chain returns, so the report must be on disk by
        // then, and a panic raised inside the logging stack could deadlock a hook that logs first.
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let name = report::panic_file_name(&self.context.run_id, seq);
        let written = report::write_atomically(&self.context.dump_dir, &name, report.render().as_bytes());

        // On desktop, when the out-of-process monitor is active, also ask it to write a minidump
        // of this panicking thread. That gives a fully symbolicatable artifact even in a stripped
        // shipping build, and it is the only way to get a dump on Windows, where `panic = "abort"`
        // terminates through `__fastfail` and bypasses the structured-exception handler. Done only
        // once: the monitor exits after writing a dump.
        if !self.dump_requested.swap(true, Ordering::Relaxed) {
            crate::request_native_panic_dump();
        }

        match written {
            Ok(path) => tracing::error!(
                target: "sapphire::crash",
                location = %report.location,
                thread = %report.thread,
                report = %path.display(),
                "panic: {}",
                report.message,
            ),
            Err(err) => tracing::error!(
                target: "sapphire::crash",
                location = %report.location,
                thread = %report.thread,
                "panic: {} (the panic report could not be written: {err})",
                report.message,
            ),
        }
    }

    fn describe(&self, info: &PanicHookInfo<'_>) -> PanicReport {
        let thread = std::thread::current();
        let thread = format!("{} ({:?})", thread.name().unwrap_or("<unnamed>"), thread.id());
        let location = info.location().map_or_else(|| "<unknown>".to_owned(), ToString::to_string);
        // Raw frame IPs and the module table so stripped builds can be symbolicated offline.
        let capture = crate::backtrace::capture();
        // The symbolized text is kept too; it is readable in a build that still has symbols.
        let backtrace = Backtrace::force_capture().to_string();
        let uptime_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        PanicReport {
            app_name: self.context.app_name.clone(),
            release: self.context.release.clone(),
            run_id: self.context.run_id.clone(),
            pid: std::process::id(),
            thread,
            location,
            unix_time_ms: report::unix_time_ms(),
            uptime_ms,
            modules: capture.modules,
            frames: capture.frames,
            message: info.payload_as_str().unwrap_or("Box<dyn Any>").to_owned(),
            backtrace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{PanicReport, ReportKind, pending_reports};
    use crate::test_support::ScratchDir;

    // The panic hook is process-wide, so this is the only test in the crate that installs it.
    #[test]
    fn writes_a_report_then_disarms() {
        let dir = ScratchDir::new("panic-hook");
        let run_id = report::new_run_id(1, std::process::id());
        let registration = install(HookContext {
            app_name: "hook-test".into(),
            release: "0.0.0".into(),
            run_id: run_id.clone(),
            dump_dir: dir.path().to_owned(),
        });

        let caught = std::thread::Builder::new()
            .name("doomed".into())
            .spawn(|| panic!("deliberate test panic {}", 42))
            .expect("spawn")
            .join();
        assert!(caught.is_err());

        let reports = pending_reports(dir.path()).expect("list");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].kind(), ReportKind::Panic);
        assert_eq!(reports[0].run_id(), run_id);
        let text = std::fs::read_to_string(reports[0].path()).expect("read report");
        let report = PanicReport::parse(&text).expect("parse report");
        assert_eq!(report.message, "deliberate test panic 42");
        assert_eq!(report.app_name, "hook-test");
        assert!(report.thread.starts_with("doomed ("), "thread: {}", report.thread);
        assert!(report.location.contains("panic_hook.rs"), "location: {}", report.location);
        assert!(!report.backtrace.is_empty());
        // Where libc exposes them, the report carries raw frame IPs and a module table with a
        // debug id for the main executable, so a stripped build is still symbolicatable.
        #[cfg(any(target_vendor = "apple", all(target_os = "linux", target_env = "gnu")))]
        {
            assert!(!report.frames.is_empty(), "expected frame IPs");
            assert!(report.frames.iter().all(|&ip| ip != 0));
            assert!(!report.modules.is_empty(), "expected a module table");
            assert!(
                report.modules.iter().any(|m| !m.debug_id.is_empty()),
                "expected at least one module debug id: {:?}",
                report.modules,
            );
        }

        drop(registration);
        let _ = std::thread::spawn(|| panic!("after disarm")).join();
        assert_eq!(pending_reports(dir.path()).expect("list").len(), 1);
    }
}
