//! The process-wide panic hook, installed on every platform.

use std::backtrace::Backtrace;
use std::panic::{self, PanicHookInfo};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Instant;

use crate::report::{self, PanicReport};

/// What the hook stamps into every report.
pub(crate) struct HookContext {
    pub app_name: String,
    pub release: String,
    pub run_id: String,
    pub dump_dir: PathBuf,
}

/// How many panics per run get a native minidump. Panics the app recovers from (an IO callback,
/// a decode worker, an off-thread shader build) each ask for one, so a panic loop would otherwise
/// fill the disk. Keep these first full reports plus one rolling raw report for the latest
/// recovered panic; later panics do not repeat expensive symbolization.
const MAX_PANIC_DUMPS: u32 = 8;

/// The backtrace section of a report written before its backtrace was symbolized. A report that
/// still says this on disk is one whose process ended while symbolizing.
const NOT_SYMBOLIZED: &str =
    "<not symbolized: the process ended while symbolizing; the frames and modules above locate the panic>\n";

const REPORT_BUDGET_EXHAUSTED: &str =
    "<not symbolized: full-report budget exhausted; this raw report replaces earlier overflow panics>\n";

/// Turns a captured backtrace into text. Tests swap it to observe the report's order of work.
type Symbolizer = Box<dyn Fn(Backtrace) -> String + Send + Sync>;

struct HookState {
    context: HookContext,
    armed: AtomicBool,
    next_seq: AtomicU32,
    /// Serializes the rolling report without making another panic wait on a panicking thread.
    latest_report: Mutex<()>,
    started: Instant,
    symbolize: Symbolizer,
    /// The player's home directory, which reports write `~` (`report::without_home`).
    home: Option<String>,
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
    install_with(context, Box::new(|backtrace| backtrace.to_string()))
}

fn install_with(context: HookContext, symbolize: Symbolizer) -> Registration {
    // Uptime in panic reports is diagnostic metadata, never simulation time (rule R2).
    #[allow(clippy::disallowed_methods)]
    let started = Instant::now();
    let state = Arc::new(HookState {
        context,
        armed: AtomicBool::new(true),
        next_seq: AtomicU32::new(0),
        latest_report: Mutex::new(()),
        started,
        symbolize,
        home: std::env::home_dir().and_then(|home| home.into_os_string().into_string().ok()),
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
        let seq = self
            .next_seq
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |seq| Some((seq + 1).min(MAX_PANIC_DUMPS)))
            .expect("the bounded sequence update always succeeds");
        // First reports have unique names. Once those are exhausted, replace one raw report.
        // Concurrent panics may skip this rolling slot while another writer owns it, rather
        // than racing on its .partial file or blocking inside a panic hook.
        let _latest = if seq == MAX_PANIC_DUMPS {
            Some(match self.latest_report.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(error)) => error.into_inner(),
                Err(TryLockError::WouldBlock) => return,
            })
        } else {
            None
        };
        // Everything in the first version of the report is read without symbolizing: the message,
        // where and when, the raw frame IPs and the modules they are in.
        let mut report = self.describe(info);
        if seq == MAX_PANIC_DUMPS {
            report.backtrace = REPORT_BUDGET_EXHAUSTED.to_owned();
        }

        // Write the file before logging or symbolizing anything. Under `panic = "abort"` (the
        // shipping profile) the process aborts as soon as the hook chain returns, so the report
        // must be on disk by then, and a panic raised inside the logging stack could deadlock a
        // hook that logs first.
        let name = report::panic_file_name(&self.context.run_id, seq);
        let written = report::write_atomically(&self.context.dump_dir, &name, report.render().as_bytes());

        // On desktop, when the out-of-process monitor is active, also ask it to write a minidump
        // of this panicking thread. That gives a fully symbolicatable artifact even in a stripped
        // shipping build, and it is the only way to get a dump on Windows, where `panic = "abort"`
        // terminates through `__fastfail` and bypasses the structured-exception handler. The
        // monitor outlives the dump, since the app may recover from this panic, and a run asks
        // for at most `MAX_PANIC_DUMPS` of them.
        if seq < MAX_PANIC_DUMPS {
            crate::request_native_panic_dump();
        }

        // Symbolize last, then replace the report with the named version. std's symbolizer reads
        // debug info from disk and walks the process's loaded images, on Apple platforms through
        // dyld's image list, which another thread unloading a library can pull from under it
        // (review PA-002). A fault or a hang there now costs only the names.
        let written = if seq < MAX_PANIC_DUMPS {
            report.backtrace = self.without_home((self.symbolize)(Backtrace::force_capture()));
            let symbolized =
                report::write_atomically(&self.context.dump_dir, &name, report.render().as_bytes());
            // If the rewrite fails, the first version is still on disk.
            symbolized.or(written)
        } else {
            written
        };

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

    /// The report without its symbolized backtrace, which [`NOT_SYMBOLIZED`] stands in for.
    fn describe(&self, info: &PanicHookInfo<'_>) -> PanicReport {
        let thread = std::thread::current();
        let thread = format!("{} ({:?})", thread.name().unwrap_or("<unnamed>"), thread.id());
        let location = info.location().map_or_else(|| "<unknown>".to_owned(), ToString::to_string);
        // Raw frame IPs and the module table so stripped builds can be symbolicated offline.
        let capture = crate::backtrace::capture();
        let uptime_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        PanicReport {
            app_name: self.context.app_name.clone(),
            release: self.context.release.clone(),
            run_id: self.context.run_id.clone(),
            pid: std::process::id(),
            thread: self.without_home(thread),
            location: self.without_home(location),
            unix_time_ms: report::unix_time_ms(),
            uptime_ms,
            modules: capture.modules,
            frames: capture.frames,
            message: self.without_home(info.payload_as_str().unwrap_or("Box<dyn Any>").to_owned()),
            backtrace: NOT_SYMBOLIZED.to_owned(),
        }
    }

    /// `text` as a report keeps it: with the player's home directory written `~`.
    fn without_home(&self, text: String) -> String {
        match &self.home {
            Some(home) => report::without_home(&text, home),
            None => text,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Mutex, PoisonError};

    use super::*;
    use crate::report::{PanicReport, ReportKind, pending_reports};
    use crate::test_support::ScratchDir;

    /// The one panic report in `dir`.
    fn only_report(dir: &Path) -> PanicReport {
        let reports = pending_reports(dir).expect("list");
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert_eq!(reports[0].kind(), ReportKind::Panic);
        let text = std::fs::read_to_string(reports[0].path()).expect("read report");
        PanicReport::parse(&text).expect("parse report")
    }

    // The panic hook is process-wide, so this is the only test in the crate that installs it.
    #[test]
    fn writes_a_report_then_disarms() {
        let dir = ScratchDir::new("panic-hook");
        let run_id = report::new_run_id(1, std::process::id());

        // PA-002: symbolizing walks the loaded images and can fault, so by the time it runs the
        // report must already be on disk. The symbolizer records what the directory held. It
        // runs inside the hook, where a panic would abort the test binary, so it only records.
        let on_disk_when_symbolizing = Arc::new(Mutex::new(None));
        let symbolizations = Arc::new(AtomicU32::new(0));
        let symbolize: Symbolizer = {
            let (dir, seen) = (dir.path().to_owned(), Arc::clone(&on_disk_when_symbolizing));
            let count = Arc::clone(&symbolizations);
            Box::new(move |backtrace| {
                count.fetch_add(1, Ordering::Relaxed);
                let report = pending_reports(&dir)
                    .ok()
                    .filter(|reports| reports.len() == 1)
                    .and_then(|reports| std::fs::read_to_string(reports[0].path()).ok())
                    .and_then(|text| PanicReport::parse(&text));
                *seen.lock().unwrap_or_else(PoisonError::into_inner) = Some(report);
                backtrace.to_string()
            })
        };
        let registration = install_with(
            HookContext {
                app_name: "hook-test".into(),
                release: "0.0.0".into(),
                run_id: run_id.clone(),
                dump_dir: dir.path().to_owned(),
            },
            symbolize,
        );

        let caught = std::thread::Builder::new()
            .name("doomed".into())
            .spawn(|| panic!("deliberate test panic {}", 42))
            .expect("spawn")
            .join();
        assert!(caught.is_err());

        let early = on_disk_when_symbolizing.lock().expect("lock").take().expect("the hook symbolized");
        let early = early.expect("the report was on disk before symbolizing");
        assert_eq!(early.message, "deliberate test panic 42");
        assert_eq!(early.backtrace, NOT_SYMBOLIZED);

        let reports = pending_reports(dir.path()).expect("list");
        assert_eq!(reports[0].run_id(), run_id);
        let report = only_report(dir.path());
        assert_eq!(report.message, "deliberate test panic 42");
        assert_eq!(report.app_name, "hook-test");
        assert!(report.thread.starts_with("doomed ("), "thread: {}", report.thread);
        assert!(report.location.contains("panic_hook.rs"), "location: {}", report.location);
        assert!(!report.backtrace.is_empty() && report.backtrace != NOT_SYMBOLIZED);
        assert_eq!((report.frames.clone(), report.modules.clone()), (early.frames, early.modules));
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

        // Recovered panic loops retain the first full reports and one latest raw report.
        for index in 1..=MAX_PANIC_DUMPS + 4 {
            let _ = std::thread::spawn(move || panic!("recovered panic {index}")).join();
        }
        let reports = pending_reports(dir.path()).expect("list");
        assert_eq!(reports.len(), MAX_PANIC_DUMPS as usize + 1);
        let latest = dir.path().join(report::panic_file_name(&run_id, MAX_PANIC_DUMPS));
        let latest =
            PanicReport::parse(&std::fs::read_to_string(latest).expect("read latest")).expect("parse latest");
        assert_eq!(latest.message, format!("recovered panic {}", MAX_PANIC_DUMPS + 4));
        assert_eq!(latest.backtrace, REPORT_BUDGET_EXHAUSTED);
        assert_eq!(symbolizations.load(Ordering::Relaxed), MAX_PANIC_DUMPS);
        assert!(on_disk_when_symbolizing.lock().expect("lock").as_ref().is_some_and(Option::is_none));

        drop(registration);
        let _ = std::thread::spawn(|| panic!("after disarm")).join();
        assert_eq!(pending_reports(dir.path()).expect("list").len(), MAX_PANIC_DUMPS as usize + 1);
    }
}
