//! Crash capture for Sapphire (spec §5.2): a panic hook on every platform, native crash
//! minidumps on desktop and an in-process signal record on Android, kept on disk for the next
//! launch to find. Uploading them to a service such as Sentry is deferred (see below).
//!
//! # What this crate owns
//!
//! - **The process-wide panic hook.** It writes a panic report (message, location, thread,
//!   timestamps, raw frame instruction pointers and the modules they are in) into the dump
//!   directory, logs the panic through `tracing`, then chains to the previous hook. The report is
//!   written synchronously before the hook returns, so it survives the shipping profile's
//!   `panic = "abort"`. It is written before the backtrace is symbolized and rewritten with the
//!   names after, so a fault while symbolizing costs only the names. The IPs and module debug ids
//!   let a stripped build be symbolicated offline (`atos -o <binary> -l <base> <ip>`), since a
//!   stripped `Backtrace` prints names without addresses. On desktop, when the monitor is
//!   active, the hook also asks it for a minidump of the panicking thread, which is the only
//!   dump Windows can produce under `panic = "abort"` (Rust aborts there through `__fastfail`,
//!   bypassing the exception handler).
//! - **Native crash capture**, per platform:
//!   - *Desktop (macOS, Windows, Linux):* [`install`] re-launches the current executable as a
//!     crash monitor and connects to it with `minidumper`. A `crash-handler` hook in the app asks
//!     the monitor to write a minidump of the crashed process from the outside, which is the
//!     robust path. The panic hook also asks the monitor for a dump of the panicking thread, so
//!     panics produce a minidump on all three desktops, Windows included. A panic the app
//!     recovers from doesn't use the monitor up: it numbers its dumps and keeps serving, so a
//!     native crash later in the run still gets one. A run asks for at most 8 panic dumps.
//!   - *Android:* `crash-handler` runs in-process and writes a small signal record (signal,
//!     fault address, registers, `/proc/self/maps`) with async-signal-safe calls only, then
//!     passes the signal on to the previous handler so `debuggerd` still writes its tombstone.
//!     There is no minidump on Android; see [Open question Q12](#open-question-q12-android).
//!   - *iOS:* none yet: only the panic hook is installed. Native crash capture on iOS needs an
//!     Apple-side reporter (for example sentry-cocoa) linked by the Xcode project (spec §5.2).
//! - **Reading back** what previous runs left ([`pending_reports`]).
//!
//! # What it deliberately does not do
//!
//! - Pick the dump directory. The app passes a writable per-user directory (for example
//!   `SDL_GetPrefPath` from `sapphire_platform`).
//! - Breadcrumbs (current zone, frame number, last 64 log lines). They need the log ring in
//!   `sapphire_core` and land with it (spec §5.2).
//! - Uploading reports. A Sentry backend existed in M0 and was removed on 2026-10-24 while
//!   nothing needs it (it doubled the crate's dependency tree); it returns, from git history,
//!   once there is a project to send reports to.
//! - Symbol upload. CI does that for shipping builds (spec §5.2, M4).
//!
//! # Wiring it into an app
//!
//! `main` must do these in order:
//!
//! 1. [`run_monitor_if_requested`] — the very first line, before anything else. In a monitor
//!    child it runs the monitor and exits; in the app it returns at once.
//! 2. Set up logging (`tracing`), so crash-capture warnings are visible.
//! 3. [`install`], with a dump directory under the platform's writable per-user directory
//!    (`sapphire_platform::user_dir`, callable before SDL starts).
//! 4. Everything else: SDL, the engine, the game.
//!
//! ```no_run
//! # use std::path::PathBuf;
//! # fn user_dir() -> PathBuf { PathBuf::from("/tmp/sapphire") }
//! fn main() {
//!     // 1. First line of main. In the monitor process this runs the monitor and exits.
//!     sapphire_crash::run_monitor_if_requested();
//!
//!     // 2. Logging.
//!     // tracing_subscriber::fmt().init();
//!
//!     // 3. Install before SDL, with the dir from sapphire_platform::user_dir(&app_metadata).
//!     let config = sapphire_crash::CrashConfig::new(
//!         "sapphire_sandbox",
//!         env!("CARGO_PKG_VERSION"),
//!         user_dir().join("crashes"),
//!     );
//!     let _crash = sapphire_crash::install(config).expect("crash capture");
//!
//!     // 4. SDL, the engine and the game run while `_crash` is alive.
//! }
//! ```
//!
//! Step 1 is not optional. If the app skips it, every monitor child runs the whole app and
//! spawns another monitor; [`install`] guards against that fork bomb by exiting when it finds
//! itself in a monitor child (see its docs). [`install`] has no dependency on SDL or any other
//! engine state, so it is safe to call this early.
//!
//! [`install`] otherwise only fails when it cannot work at all (already installed, or the dump
//! directory cannot be created). If native capture cannot start, for example because the monitor
//! fails to launch, it logs a warning and carries on with the panic hook alone;
//! [`CrashGuard::native_capture`] tells which kind of capture is running.
//!
//! ## A dedicated monitor executable (later)
//!
//! Re-executing the app as its own monitor is enough for M0. A separate, tiny monitor executable
//! would be more robust for DRM-wrapped or packed binaries (where `current_exe` is the wrapper)
//! and for load-time-injected DLLs, and it is the natural home for the Android monitor that must
//! ship as a `lib*.so` (Q12). It is deliberately deferred; the env-var guard above makes the
//! re-exec approach safe in the meantime.
//!
//! # Open question Q12: Android
//!
//! *Does `minidumper` work on Android, or should the engine write dumps in-process or use
//! Sentry's Android NDK SDK?* What the crates support, read from their sources:
//!
//! - `crash-handler` 0.8 supports Android. It installs the signal handlers with an alternate
//!   stack and works around Android's `sigaction` quirk. This crate uses it.
//! - `minidumper` 0.11 compiles for Android (it shares the Linux socket code), but it needs a
//!   separate monitor process, and an Android app has no executable of its own to launch:
//!   `current_exe()` is `/system/bin/app_process64`. Since API 29 apps may not execute files
//!   from writable storage either. The only route is to ship a second executable disguised as a
//!   `lib*.so` in the APK's native library directory, as Crashpad does.
//! - `minidump-writer` 0.13 (the writer behind `minidumper`) reads the crashed process with
//!   `ptrace`, which a process cannot do to its own threads, so it cannot dump in-process. Its
//!   README lists `aarch64-linux-android` as "implemented, but untested". Breakpad's way out
//!   (clone a child at crash time that ptraces the parent) needs an allocation-free writer,
//!   and `minidump-writer` allocates.
//!
//! **Answer:** these crates give no minidumps on Android. M0 ships the in-process signal record,
//! which detects the crash and keeps the faulting PC, LR and module map for `addr2line`, while
//! `debuggerd` tombstones cover development. Symbolicated native crashes in a crash service should
//! come from that service's Android NDK SDK (for Sentry, `sentry-android-ndk` in the Gradle
//! project). Both can coexist: this crate's handler hands the signal on to whichever handler
//! was installed before it.

mod backtrace;
mod panic_hook;
mod report;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
mod monitor;

#[cfg(target_os = "android")]
mod android;

#[cfg(any(target_os = "android", test))]
mod signal_record;

#[cfg(test)]
mod test_support;

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

pub use report::{PendingReport, ReportKind, pending_reports};

/// What [`install`] needs to know about the app.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CrashConfig {
    /// The app's name, recorded in every report.
    pub app_name: String,
    /// Version or build ID, recorded in every report.
    pub release: String,
    /// A writable per-user directory for minidumps and panic reports. Created if missing.
    pub dump_dir: PathBuf,
}

impl CrashConfig {
    /// A configuration for `app_name` at `release`, keeping reports in `dump_dir`.
    pub fn new(
        app_name: impl Into<String>,
        release: impl Into<String>,
        dump_dir: impl Into<PathBuf>,
    ) -> Self {
        Self { app_name: app_name.into(), release: release.into(), dump_dir: dump_dir.into() }
    }
}

/// Errors from this crate's public API.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CrashError {
    /// [`install`] was called while a [`CrashGuard`] is still alive.
    #[error("crash capture is already installed in this process")]
    AlreadyInstalled,
    /// The dump directory could not be created.
    #[error("cannot create the dump directory {}", .path.display())]
    CreateDumpDir {
        /// The directory from [`CrashConfig::dump_dir`].
        path: PathBuf,
        /// What the OS reported.
        #[source]
        source: io::Error,
    },
    /// The dump directory could not be listed.
    #[error("cannot read the dump directory {}", .path.display())]
    ReadDumpDir {
        /// The directory passed to [`pending_reports`].
        path: PathBuf,
        /// What the OS reported.
        #[source]
        source: io::Error,
    },
}

/// Which native (non-panic) crash capture is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeCapture {
    /// Desktop: a monitor process writes a minidump when the app crashes (`<run>.dmp`, or
    /// `<run>.<n>.dmp` after the run's panics have asked for dumps of their own).
    Minidump,
    /// Android: an in-process signal handler writes a signal record (`<run>.native.txt`).
    SignalRecord,
    /// Panics only: on iOS (native capture comes with the Xcode project), because native capture
    /// failed to start, in which case [`install`] logged why, or because the desktop monitor has
    /// since gone away.
    None,
}

/// Keeps crash capture alive. Dropping it shuts the monitor down and disarms the panic hook.
#[must_use = "crash capture stops when the guard is dropped"]
pub struct CrashGuard {
    native: Native,
    panic_hook: Option<panic_hook::Registration>,
    run_id: String,
}

impl CrashGuard {
    /// Which native crash capture is running now. On desktop that is
    /// [`NativeCapture::Minidump`] only while the monitor process is alive and connected, so a
    /// native crash at this moment would get a dump; recovered panics don't change it.
    pub fn native_capture(&self) -> NativeCapture {
        match &self.native {
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            Native::Monitor { capture } if capture.is_active() => NativeCapture::Minidump,
            #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
            Native::Monitor { .. } => NativeCapture::None,
            #[cfg(target_os = "android")]
            Native::Signal { .. } => NativeCapture::SignalRecord,
            Native::None => NativeCapture::None,
        }
    }

    /// This run's ID. Every report the run writes starts with it (see [`PendingReport::run_id`]).
    pub fn run_id(&self) -> &str {
        &self.run_id
    }
}

impl Drop for CrashGuard {
    fn drop(&mut self) {
        // Native capture goes first, so the monitor has exited before `install` can run again.
        self.native = Native::None;
        self.panic_hook = None;
        INSTALLED.store(false, Ordering::Release);
    }
}

/// The running native capture. Dropping its payload stops the capture.
enum Native {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    Monitor {
        capture: monitor::MonitorCapture,
    },
    #[cfg(target_os = "android")]
    Signal {
        _capture: android::SignalCapture,
    },
    None,
}

/// Set while a [`CrashGuard`] is alive. `crash-handler` allows only one handler per process.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Environment variable set on the spawned crash monitor. It guards against a self-spawn fork
/// bomb: if a process that has it reaches [`install`], the app launched a monitor child but did
/// not call [`run_monitor_if_requested`] first, so the child ran the whole app and would spawn
/// another monitor, and so on. [`install`] detects the variable and exits instead.
pub(crate) const MONITOR_ENV: &str = "SAPPHIRE_CRASH_MONITOR";

/// The exit code [`install`] ends a misbehaving monitor child with (its
/// [panic-monitor guard](install#panic-monitor-guard)). Distinct from a Rust panic (101) and from
/// the demo's own codes, so it is recognisable in logs and by whatever launched the process.
pub const MONITOR_GUARD_EXIT_CODE: i32 = 70;

/// Whether this process was launched as a crash monitor, from the value of [`MONITOR_ENV`].
fn launched_as_monitor(env_value: Option<&std::ffi::OsStr>) -> bool {
    env_value.is_some_and(|value| !value.is_empty())
}

/// Runs the crash monitor and exits, if this process was launched as one; otherwise returns at
/// once.
///
/// Call it as the very first line of `main`, before SDL or anything else starts: on desktop,
/// [`install`] launches the monitor by re-running the current executable with a private
/// command-line flag. On Android and iOS there is no monitor and this always returns.
pub fn run_monitor_if_requested() {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    monitor::run_if_requested();
}

/// Requests a native minidump of the panicking thread, on desktop with the monitor active.
///
/// Returns `false` if there is no native handler to ask (iOS, Android, or the monitor failed to
/// start or has gone) or the dump could not be written. Used only by the panic hook.
fn request_native_panic_dump() -> bool {
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    {
        monitor::request_panic_dump()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

/// Installs the panic hook and this platform's native crash capture.
///
/// Reports go to [`CrashConfig::dump_dir`], which is created if needed. Keep the returned guard
/// alive for as long as crashes should be captured, normally until the end of `main`.
///
/// # Panic-monitor guard
///
/// If this process was launched as a crash monitor (its environment carries the monitor's
/// marker, `SAPPHIRE_CRASH_MONITOR`) but reached `install`, the app did not call
/// [`run_monitor_if_requested`] first. Returning would make the monitor child run the whole app
/// and spawn its own monitor, a fork bomb. Instead `install` logs, prints to stderr and exits
/// the process with [`MONITOR_GUARD_EXIT_CODE`]; it does not return and does not spawn.
///
/// # Errors
///
/// [`CrashError::AlreadyInstalled`] if another guard is alive, and
/// [`CrashError::CreateDumpDir`] if the dump directory cannot be created. Failing to start
/// native capture is not an error: it is logged and [`CrashGuard::native_capture`] reports
/// [`NativeCapture::None`].
pub fn install(config: CrashConfig) -> Result<CrashGuard, CrashError> {
    if launched_as_monitor(std::env::var_os(MONITOR_ENV).as_deref()) {
        tracing::error!(
            target: "sapphire::crash",
            "this process is a crash monitor but reached install(); call run_monitor_if_requested() \
             as the first line of main. Exiting to avoid a self-spawn fork bomb.",
        );
        // tracing may not be set up this early, so also go straight to stderr.
        eprintln!(
            "sapphire_crash: process launched as a crash monitor reached install(); \
             run_monitor_if_requested() must be the first line of main. Exiting.",
        );
        std::process::exit(MONITOR_GUARD_EXIT_CODE);
    }

    if INSTALLED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return Err(CrashError::AlreadyInstalled);
    }
    let result = install_inner(config);
    if result.is_err() {
        INSTALLED.store(false, Ordering::Release);
    }
    result
}

fn install_inner(config: CrashConfig) -> Result<CrashGuard, CrashError> {
    std::fs::create_dir_all(&config.dump_dir)
        .map_err(|source| CrashError::CreateDumpDir { path: config.dump_dir.clone(), source })?;
    let run_id = report::new_run_id(report::unix_time_ms(), std::process::id());

    // Sweep away `.partial` files from runs that died mid-write, before this run writes anything.
    report::cleanup_stale_partials(&config.dump_dir, &run_id);

    // The hook goes in first so that a panic while starting native capture is still reported.
    let panic_hook = panic_hook::install(panic_hook::HookContext {
        app_name: config.app_name.clone(),
        release: config.release.clone(),
        run_id: run_id.clone(),
        dump_dir: config.dump_dir.clone(),
    });
    let guard = CrashGuard { native: start_native(&config, &run_id), panic_hook: Some(panic_hook), run_id };

    tracing::info!(
        target: "sapphire::crash",
        run = %guard.run_id,
        native = ?guard.native_capture(),
        dir = %config.dump_dir.display(),
        "crash capture installed",
    );
    Ok(guard)
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn start_native(config: &CrashConfig, run_id: &str) -> Native {
    match monitor::MonitorCapture::start(&config.dump_dir, run_id) {
        Ok(capture) => Native::Monitor { capture },
        Err(err) => {
            tracing::warn!(
                target: "sapphire::crash",
                "native crash capture is unavailable, only panics will be reported: {err}",
            );
            Native::None
        }
    }
}

#[cfg(target_os = "android")]
fn start_native(config: &CrashConfig, run_id: &str) -> Native {
    match android::SignalCapture::start(config, run_id) {
        Ok(capture) => Native::Signal { _capture: capture },
        Err(err) => {
            tracing::warn!(
                target: "sapphire::crash",
                "native crash capture is unavailable, only panics will be reported: {err}",
            );
            Native::None
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux", target_os = "android")))]
fn start_native(_config: &CrashConfig, _run_id: &str) -> Native {
    // iOS: native crashes need an Apple-side reporter linked by the Xcode project (spec §5.2).
    Native::None
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn monitor_env_decides_the_self_spawn_guard() {
        // A set, non-empty value means "this process is a monitor child": install() must exit.
        assert!(launched_as_monitor(Some(OsStr::new("1"))));
        assert!(launched_as_monitor(Some(OsStr::new("anything"))));
        // Unset or empty means a normal app launch: install() proceeds.
        assert!(!launched_as_monitor(None));
        assert!(!launched_as_monitor(Some(OsStr::new(""))));
    }

    #[test]
    fn guard_exit_code_is_distinct() {
        // Not a Rust panic (101) or the demo's usage code (2), so it stands out in logs.
        assert_ne!(MONITOR_GUARD_EXIT_CODE, 101);
        assert_ne!(MONITOR_GUARD_EXIT_CODE, 2);
        assert_ne!(MONITOR_GUARD_EXIT_CODE, 0);
    }
}
