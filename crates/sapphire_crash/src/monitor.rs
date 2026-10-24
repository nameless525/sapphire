//! Desktop native crash capture: a monitor process writes minidumps of this one (spec §5.2).
//!
//! [`MonitorCapture::start`] re-launches the current executable with [`MONITOR_FLAG`], so the
//! monitor is the app's own binary and nothing extra ships. That process lands in
//! [`run_if_requested`] at the top of `main` and serves a `minidumper` socket. When the app
//! crashes, `crash-handler` sends the crash context over the socket and blocks until the monitor
//! has written `<run>.dmp`.
//!
//! The monitor's stdin is a pipe whose write end only the app holds, and the app never writes
//! to it. When the app goes away for any reason (clean exit, crash after its dump, `SIGKILL`),
//! the OS closes the pipe and the monitor sees end-of-file and exits. No polling, no pid checks.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crash_handler::{CrashEventResult, CrashHandler};
use minidumper::{Client, LoopAction, MinidumpBinary, Server, ServerHandler, SocketName};

use crate::report;

/// The private flag that turns a launch of the app's executable into a crash monitor.
const MONITOR_FLAG: &str = "--sapphire-crash-monitor";

/// How long the app waits for a freshly launched monitor to accept its connection (5 s).
const CONNECT_ATTEMPTS: u32 = 250;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(20);

/// How long [`MonitorCapture`]'s drop waits for the monitor to exit before killing it (2 s).
const EXIT_POLLS: u32 = 200;
const EXIT_POLL_DELAY: Duration = Duration::from_millis(10);

/// `sun_path` holds 104 bytes on macOS and 108 on Windows, including the terminating NUL.
#[cfg(not(target_os = "linux"))]
const MAX_SOCKET_PATH_LEN: usize = 103;

/// Why native capture could not start. Only ever logged: [`crate::install`] falls back to panics.
#[derive(Debug, thiserror::Error)]
pub(crate) enum MonitorError {
    #[error("cannot locate the current executable: {0}")]
    CurrentExe(io::Error),
    #[cfg(not(target_os = "linux"))]
    #[error("cannot name the monitor socket: {0}")]
    SocketName(String),
    #[error("cannot launch the crash monitor: {0}")]
    Spawn(io::Error),
    #[error("the crash monitor exited before accepting a connection ({0})")]
    MonitorExited(ExitStatus),
    #[error("cannot connect to the crash monitor: {0}")]
    Connect(minidumper::Error),
    #[error("cannot attach the crash handler: {0}")]
    Handler(crash_handler::Error),
}

/// The crash handler of the active monitor, so the panic hook can ask it for a dump of the
/// panicking thread (`request_panic_dump`). It is an `Arc` shared with the owning
/// [`MonitorCapture`]; clearing it here plus dropping the capture's clone detaches the handler.
static PANIC_DUMP_HANDLER: Mutex<Option<Arc<CrashHandler>>> = Mutex::new(None);

/// The app side: a running monitor plus the crash handler that talks to it.
pub(crate) struct MonitorCapture {
    handler: Option<Arc<CrashHandler>>,
    monitor: Child,
    lifeline: Option<ChildStdin>,
}

impl MonitorCapture {
    pub(crate) fn start(dump_dir: &Path, run_id: &str) -> Result<Self, MonitorError> {
        let exe = std::env::current_exe().map_err(MonitorError::CurrentExe)?;
        let socket = SocketId::new(run_id)?;
        let mut monitor = Command::new(exe)
            .arg(MONITOR_FLAG)
            .arg(socket.as_str())
            .arg(dump_dir)
            .arg(run_id)
            // The guard that stops a self-spawn fork bomb if the app forgot
            // `run_monitor_if_requested()`: a child that reaches `install()` sees this and exits
            // (see `crate::install`). It is also inherited by any process the monitor spawns.
            .env(crate::MONITOR_ENV, "1")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(MonitorError::Spawn)?;
        let lifeline = monitor.stdin.take();

        let handler = connect(&socket, &mut monitor).and_then(|client| {
            // SAFETY: the event runs in a crashed process (inside a signal handler on Linux), so
            // it must not allocate or take locks the crashing code might hold. It only calls
            // `Client::request_dump`, which sends a fixed-size message over the already-connected
            // socket (a Mach message on macOS) and blocks on the monitor's acknowledgement.
            let event = unsafe {
                crash_handler::make_crash_event(move |context| {
                    CrashEventResult::Handled(client.request_dump(context).is_ok())
                })
            };
            CrashHandler::attach(event).map_err(MonitorError::Handler)
        });
        let handler = match handler {
            Ok(handler) => Arc::new(handler),
            Err(err) => {
                // Dropping the lifeline would stop it too, but don't leave it to chance.
                let _ = monitor.kill();
                let _ = monitor.wait();
                return Err(err);
            }
        };

        // With Yama's `ptrace_scope = 1`, only the process named here may read our memory.
        #[cfg(target_os = "linux")]
        handler.set_ptracer(Some(monitor.id()));

        // Publish the handler so the panic hook can request a dump of a panicking thread.
        *lock_handler() = Some(Arc::clone(&handler));

        Ok(Self { handler: Some(handler), monitor, lifeline })
    }
}

impl Drop for MonitorCapture {
    fn drop(&mut self) {
        // Detach first, so no crash request can reach a monitor that is shutting down. The handler
        // detaches when its last `Arc` drops, so clear the shared slot before the owned clone.
        *lock_handler() = None;
        self.handler = None;
        // Closing the lifeline tells the monitor to exit.
        self.lifeline = None;
        for _ in 0..EXIT_POLLS {
            match self.monitor.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => thread::sleep(EXIT_POLL_DELAY),
                Err(_) => break,
            }
        }
        tracing::warn!(target: "sapphire::crash", "the crash monitor did not shut down cleanly; killing it");
        let _ = self.monitor.kill();
        let _ = self.monitor.wait();
    }
}

fn lock_handler() -> std::sync::MutexGuard<'static, Option<Arc<CrashHandler>>> {
    // The slot only holds a handle; a poisoned lock carries no broken invariant.
    PANIC_DUMP_HANDLER.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Asks the active monitor to write a minidump of the calling (panicking) thread.
///
/// Returns `false` if no monitor is running. The crash handler's `simulate_*` path runs our crash
/// event, which requests the dump over the existing connection and blocks until it is written;
/// the monitor then exits, so this is meaningful only once per run.
pub(crate) fn request_panic_dump() -> bool {
    let guard = lock_handler();
    let Some(handler) = guard.as_ref() else {
        return false;
    };
    #[cfg(target_os = "linux")]
    {
        use crash_handler::Signal;
        matches!(handler.simulate_signal(Signal::Abort as u32), CrashEventResult::Handled(true))
    }
    #[cfg(target_os = "macos")]
    {
        handler.simulate_exception(None)
    }
    #[cfg(target_os = "windows")]
    {
        matches!(handler.simulate_exception(None), CrashEventResult::Handled(true))
    }
}

/// Connects to the monitor, retrying while it starts up.
fn connect(socket: &SocketId, monitor: &mut Child) -> Result<Client, MonitorError> {
    let mut attempt = 1;
    loop {
        match Client::with_name(socket.name()) {
            Ok(client) => return Ok(client),
            Err(err) if attempt >= CONNECT_ATTEMPTS => return Err(MonitorError::Connect(err)),
            Err(_) => {}
        }
        if let Ok(Some(status)) = monitor.try_wait() {
            return Err(MonitorError::MonitorExited(status));
        }
        attempt += 1;
        thread::sleep(CONNECT_RETRY_DELAY);
    }
}

/// The monitor's socket name, unique per run: an abstract socket on Linux (nothing to clean up
/// on disk) and a path in the temp directory elsewhere. On macOS the path doubles as the name of
/// the Mach port that carries the crash context.
struct SocketId(String);

impl SocketId {
    fn new(run_id: &str) -> Result<Self, MonitorError> {
        let name = format!("sapphire-crash-{run_id}.sock");
        #[cfg(target_os = "linux")]
        {
            Ok(Self(name))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let path = std::env::temp_dir().join(name);
            let path = path.into_os_string().into_string().map_err(|path| {
                MonitorError::SocketName(format!("{} is not UTF-8", Path::new(&path).display()))
            })?;
            if path.len() > MAX_SOCKET_PATH_LEN {
                return Err(MonitorError::SocketName(format!("{path} is too long for a Unix socket")));
            }
            Ok(Self(path))
        }
    }

    fn as_str(&self) -> &str {
        &self.0
    }

    fn name(&self) -> SocketName<'_> {
        #[cfg(target_os = "linux")]
        {
            SocketName::abstract_namespace(&self.0)
        }
        #[cfg(not(target_os = "linux"))]
        {
            SocketName::path(&self.0)
        }
    }
}

/// The monitor's command line, after [`MONITOR_FLAG`].
#[derive(Debug, PartialEq, Eq)]
struct MonitorArgs {
    socket: String,
    dump_dir: PathBuf,
    run_id: String,
}

impl MonitorArgs {
    fn parse(mut args: impl Iterator<Item = OsString>) -> Option<Self> {
        let socket = args.next()?.into_string().ok()?;
        let dump_dir = PathBuf::from(args.next()?);
        let run_id = args.next()?.into_string().ok()?;
        if args.next().is_some() || socket.is_empty() || !report::is_valid_run_id(&run_id) {
            return None;
        }
        Some(Self { socket, dump_dir, run_id })
    }
}

/// The monitor side: if this process is a monitor, serve until the app is gone, then exit.
pub(crate) fn run_if_requested() {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(OsStr::new(MONITOR_FLAG)) {
        return;
    }
    // A monitor runs before the app sets up logging, so it has no `tracing` subscriber. Its
    // stderr is the app's, which is where these rare messages are most likely to be seen.
    let code = match MonitorArgs::parse(args) {
        Some(args) => match serve(args) {
            Ok(()) => 0,
            Err(err) => {
                eprintln!("sapphire_crash monitor: {err}");
                1
            }
        },
        None => {
            eprintln!("sapphire_crash monitor: usage: {MONITOR_FLAG} <socket> <dump-dir> <run-id>");
            2
        }
    };
    std::process::exit(code);
}

fn serve(args: MonitorArgs) -> Result<(), minidumper::Error> {
    let socket = SocketId(args.socket);
    let mut server = Server::with_name(socket.name())?;
    let shutdown = Arc::new(AtomicBool::new(false));
    watch_lifeline(Arc::clone(&shutdown))?;
    let writer = DumpWriter { dump_dir: args.dump_dir, run_id: args.run_id };
    server.run(Box::new(writer), &shutdown, None)
}

/// Sets `shutdown` once stdin reaches end-of-file, which happens when the app is gone.
fn watch_lifeline(shutdown: Arc<AtomicBool>) -> io::Result<()> {
    thread::Builder::new().name("sapphire-crash-lifeline".into()).spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0u8; 64];
        loop {
            match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        shutdown.store(true, Ordering::Relaxed);
    })?;
    Ok(())
}

/// Writes the app's minidump to `<run>.dmp` (through `<run>.dmp.partial`).
struct DumpWriter {
    dump_dir: PathBuf,
    run_id: String,
}

impl DumpWriter {
    fn final_path(&self) -> PathBuf {
        self.dump_dir.join(report::minidump_file_name(&self.run_id))
    }

    fn finish(&self, dump: MinidumpBinary) -> io::Result<PathBuf> {
        dump.file.sync_all()?;
        let final_path = self.final_path();
        fs::rename(&dump.path, &final_path)?;
        Ok(final_path)
    }
}

impl ServerHandler for DumpWriter {
    fn create_minidump_file(&self) -> Result<(File, PathBuf), io::Error> {
        let path = self.dump_dir.join(report::partial_name(&report::minidump_file_name(&self.run_id)));
        let file = File::create(&path)?;
        Ok((file, path))
    }

    fn on_minidump_created(&self, result: Result<MinidumpBinary, minidumper::Error>) -> LoopAction {
        let partial = self.dump_dir.join(report::partial_name(&report::minidump_file_name(&self.run_id)));
        match result.map_err(io::Error::other).and_then(|dump| self.finish(dump)) {
            Ok(_) => {}
            Err(err) => {
                eprintln!("sapphire_crash monitor: failed to write the minidump: {err}");
                let _ = fs::remove_file(partial);
            }
        }
        // The app has crashed; there is nothing left to monitor.
        LoopAction::Exit
    }

    fn on_message(&self, _kind: u32, _buffer: Vec<u8>) {
        // The app sends no messages yet. App state for dumps comes with breadcrumbs (spec §5.2).
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = OsString> {
        list.iter().map(OsString::from).collect::<Vec<_>>().into_iter()
    }

    #[test]
    fn parses_monitor_arguments() {
        let parsed = MonitorArgs::parse(args(&["/tmp/s.sock", "/data/crashes", "0001761300000123-42"]));
        assert_eq!(
            parsed,
            Some(MonitorArgs {
                socket: "/tmp/s.sock".into(),
                dump_dir: PathBuf::from("/data/crashes"),
                run_id: "0001761300000123-42".into(),
            })
        );
    }

    #[test]
    fn rejects_malformed_monitor_arguments() {
        assert_eq!(MonitorArgs::parse(args(&[])), None);
        assert_eq!(MonitorArgs::parse(args(&["s", "/d"])), None);
        assert_eq!(MonitorArgs::parse(args(&["s", "/d", "../../etc"])), None);
        assert_eq!(MonitorArgs::parse(args(&["", "/d", "1-2"])), None);
        assert_eq!(MonitorArgs::parse(args(&["s", "/d", "1-2", "extra"])), None);
    }

    #[test]
    fn socket_names_fit_the_platform_limit() {
        let socket = SocketId::new(&report::new_run_id(u64::MAX / 1000, u32::MAX)).expect("socket name");
        #[cfg(not(target_os = "linux"))]
        assert!(socket.as_str().len() <= MAX_SOCKET_PATH_LEN);
        assert!(socket.as_str().contains("sapphire-crash-"));
    }

    #[test]
    fn run_if_requested_returns_for_a_normal_launch() {
        // The test harness is not launched with the monitor flag, so this must return.
        run_if_requested();
    }
}
