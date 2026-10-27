//! Desktop native crash capture: a monitor process writes minidumps of this one (spec §5.2).
//!
//! [`MonitorCapture::start`] re-launches the current executable with [`MONITOR_FLAG`], so the
//! monitor is the app's own binary and nothing extra ships. That process lands in
//! [`run_if_requested`] at the top of `main` and serves a `minidumper` socket. When the app
//! crashes, `crash-handler` sends the crash context over the socket and blocks until the monitor
//! has written `<run>.dmp`.
//!
//! A panic the app recovers from (an IO callback, a decode worker, an off-thread shader build)
//! also asks for a dump, so the monitor outlives its dumps: it numbers them (`<run>.dmp`, then
//! `<run>.1.dmp`, `<run>.2.dmp`…) and keeps serving, and the app reconnects after each panic dump
//! (see `Connection`), so a native crash later in the run still gets its own dump.
//!
//! The monitor's stdin is a pipe whose write end only the app holds, and the app never writes
//! to it. When the app goes away for any reason (clean exit, crash after its dump, `SIGKILL`),
//! the OS closes the pipe and the monitor sees end-of-file and exits. No polling, no pid checks.

use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, LocalKey};
use std::time::Duration;

use crash_handler::{CrashEventResult, CrashHandler};
use minidumper::{Client, LoopAction, MinidumpBinary, Server, ServerHandler, SocketName};

use crate::report;

/// The private flag that turns a launch of the app's executable into a crash monitor.
const MONITOR_FLAG: &str = "--sapphire-crash-monitor";

/// How long the app waits for a freshly launched monitor to accept its connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long [`MonitorCapture`]'s drop waits for the monitor to exit before killing it.
const EXIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Waiting on the monitor polls with exponential backoff from [`FIRST_POLL_DELAY`] up to
/// [`MAX_POLL_DELAY`]: the monitor is usually ready, or gone, within a few milliseconds, which a
/// fixed 20 ms delay turned into a 20–40 ms wait at every launch and exit.
const FIRST_POLL_DELAY: Duration = Duration::from_millis(1);
const MAX_POLL_DELAY: Duration = Duration::from_millis(20);

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

/// The link to the active monitor, so the panic hook can ask it for a dump of the panicking
/// thread (`request_panic_dump`). [`MonitorCapture`] fills it at start and empties it on drop,
/// which detaches the crash handler.
static LINK: Mutex<Option<Link>> = Mutex::new(None);

thread_local! {
    /// Set while this thread asks the monitor for a panic dump, so a panic raised inside that
    /// request (in crash-handler or minidumper) doesn't ask again and deadlock on [`LINK`].
    static REQUESTING: Cell<bool> = const { Cell::new(false) };
}

/// The app side: a running monitor plus the crash handler that talks to it (in [`LINK`]).
pub(crate) struct MonitorCapture {
    /// Behind a lock so `is_active` can poll it through `&self`.
    monitor: Mutex<Child>,
    lifeline: Option<ChildStdin>,
}

/// The crash handler and the connection its crash event requests dumps through.
struct Link {
    handler: CrashHandler,
    connection: Arc<Connection>,
    socket: SocketId,
}

/// The monitor connection the crash event sends its dump request through.
///
/// minidumper's server closes a connection once it has written a dump for it, and keeps serving.
/// So after a panic dump the app connects again and swaps the new connection in, which keeps
/// native capture working after a recovered panic. The crash event reads the current connection
/// with one atomic load, which is safe in a signal handler. A replaced connection is retired
/// rather than freed, because a crash on another thread may be using it; retired connections
/// (at most one per panic dump) are freed with the handler.
struct Connection {
    current: AtomicPtr<Client>,
    /// Boxed so a retired client stays at the address a crash event may have loaded.
    #[allow(clippy::vec_box)]
    retired: Mutex<Vec<Box<Client>>>,
}

impl Connection {
    fn new(client: Client) -> Self {
        Self { current: AtomicPtr::new(Box::into_raw(Box::new(client))), retired: Mutex::new(Vec::new()) }
    }

    /// Sends a crash's dump request and waits until the monitor has written the dump.
    /// Async-signal-safe: no allocation and no locks.
    fn request_dump(&self, context: &crash_handler::CrashContext) -> bool {
        // SAFETY: a non-null pointer here came from `Box::into_raw` and is freed only in `drop`,
        // after the crash handler holding this connection has been detached: a replaced client
        // moves to `retired` instead.
        let client = unsafe { self.current.load(Ordering::Acquire).as_ref() };
        client.is_some_and(|client| client.request_dump(context).is_ok())
    }

    fn is_connected(&self) -> bool {
        !self.current.load(Ordering::Acquire).is_null()
    }

    /// Makes `client` the connection crash events use; `None` leaves them none.
    fn replace(&self, client: Option<Client>) {
        let new = client.map_or(ptr::null_mut(), |client| Box::into_raw(Box::new(client)));
        let old = self.current.swap(new, Ordering::AcqRel);
        if !old.is_null() {
            // SAFETY: `old` came from `Box::into_raw` and was just unpublished; it is retired, not
            // freed, because a crash event on another thread may have loaded it already.
            let old = unsafe { Box::from_raw(old) };
            self.retired.lock().unwrap_or_else(|poison| poison.into_inner()).push(old);
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let current = *self.current.get_mut();
        if !current.is_null() {
            // SAFETY: dropping the last `Arc` means the crash handler (whose event held the other)
            // is detached, so nothing can be using the client any more.
            drop(unsafe { Box::from_raw(current) });
        }
    }
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

        let attached = connect(&socket, &mut monitor).and_then(|client| {
            let connection = Arc::new(Connection::new(client));
            let event_connection = Arc::clone(&connection);
            // SAFETY: the event runs in a crashed process (inside a signal handler on Linux), so
            // it must not allocate or take locks the crashing code might hold. It only loads the
            // current connection atomically and calls `Client::request_dump`, which sends a
            // fixed-size message over the already-connected socket (a Mach message on macOS) and
            // blocks on the monitor's acknowledgement.
            let event = unsafe {
                crash_handler::make_crash_event(move |context| {
                    CrashEventResult::Handled(event_connection.request_dump(context))
                })
            };
            let handler = CrashHandler::attach(event).map_err(MonitorError::Handler)?;
            Ok((handler, connection))
        });
        let (handler, connection) = match attached {
            Ok(attached) => attached,
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

        // Publish the link so the panic hook can request a dump of a panicking thread.
        *lock_link() = Some(Link { handler, connection, socket });

        Ok(Self { monitor: Mutex::new(monitor), lifeline })
    }

    /// Whether a native crash would get a dump now: the monitor is running and the crash
    /// handler has a connection to it.
    pub(crate) fn is_active(&self) -> bool {
        let connected = lock_link().as_ref().is_some_and(|link| link.connection.is_connected());
        let mut monitor = self.monitor.lock().unwrap_or_else(|poison| poison.into_inner());
        connected && matches!(monitor.try_wait(), Ok(None))
    }
}

impl Drop for MonitorCapture {
    fn drop(&mut self) {
        // Detach first, so no crash request can reach a monitor that is shutting down.
        drop(lock_link().take());
        // Closing the lifeline tells the monitor to exit.
        self.lifeline = None;
        let monitor = self.monitor.get_mut().unwrap_or_else(|poison| poison.into_inner());
        let mut backoff = Backoff::new(EXIT_TIMEOUT);
        loop {
            match monitor.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if backoff.sleep() => {}
                Ok(None) | Err(_) => break,
            }
        }
        tracing::warn!(target: "sapphire::crash", "the crash monitor did not shut down cleanly; killing it");
        let _ = monitor.kill();
        let _ = monitor.wait();
    }
}

fn lock_link() -> MutexGuard<'static, Option<Link>> {
    // The slot only holds handles; a poisoned lock carries no broken invariant.
    LINK.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// Asks the active monitor to write a minidump of the calling (panicking) thread, then
/// reconnects, so later panics and native crashes still reach the monitor.
///
/// Returns whether the dump was written: `false` without a monitor, or when this thread is
/// already asking (a panic inside the request). The crash handler's `simulate_*` path runs our
/// crash event, which requests the dump and blocks until it is written. The monitor keeps
/// running, but closes the connection the dump came through, so this connects again; if that
/// fails, the monitor is gone and native capture is off (`is_active`).
pub(crate) fn request_panic_dump() -> bool {
    if REQUESTING.try_with(|requesting| requesting.replace(true)).unwrap_or(true) {
        return false;
    }
    let _reset = ResetOnDrop(&REQUESTING);
    let guard = lock_link();
    let Some(link) = guard.as_ref() else {
        return false;
    };
    let dumped = simulate_crash(&link.handler);
    if dumped && cfg!(panic = "abort") {
        // Under `panic = "abort"` (the shipping profile) no panic is recovered: the abort that
        // follows would only write this dump again, so leave the crash event no connection.
        link.connection.replace(None);
        return dumped;
    }
    match Client::with_name(link.socket.name()) {
        Ok(client) => link.connection.replace(Some(client)),
        Err(err) => {
            link.connection.replace(None);
            tracing::warn!(
                target: "sapphire::crash",
                "cannot reconnect to the crash monitor after a panic dump; native crash capture is off: {err}",
            );
        }
    }
    dumped
}

/// Clears a thread-local flag when dropped, unwinding included.
struct ResetOnDrop(&'static LocalKey<Cell<bool>>);

impl Drop for ResetOnDrop {
    fn drop(&mut self) {
        let _ = self.0.try_with(|flag| flag.set(false));
    }
}

/// Runs the crash event for the calling thread as if it had crashed.
fn simulate_crash(handler: &CrashHandler) -> bool {
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
    let mut backoff = Backoff::new(CONNECT_TIMEOUT);
    loop {
        let error = match Client::with_name(socket.name()) {
            Ok(client) => return Ok(client),
            Err(error) => error,
        };
        if let Ok(Some(status)) = monitor.try_wait() {
            return Err(MonitorError::MonitorExited(status));
        }
        if !backoff.sleep() {
            return Err(MonitorError::Connect(error));
        }
    }
}

/// Sleeps between polls, doubling the delay from [`FIRST_POLL_DELAY`] up to [`MAX_POLL_DELAY`],
/// until `timeout` worth of sleeping has passed. Counting sleeps rather than reading a clock
/// keeps the OS clock out of this crate (rule R2); the timeout is a safety net, not a deadline.
struct Backoff {
    delay: Duration,
    slept: Duration,
    timeout: Duration,
}

impl Backoff {
    fn new(timeout: Duration) -> Self {
        Self { delay: FIRST_POLL_DELAY, slept: Duration::ZERO, timeout }
    }

    /// Sleeps before the next poll, or returns `false` once the timeout is used up.
    fn sleep(&mut self) -> bool {
        if self.slept >= self.timeout {
            return false;
        }
        thread::sleep(self.delay);
        self.slept += self.delay;
        self.delay = (self.delay * 2).min(MAX_POLL_DELAY);
        true
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
    let writer = DumpWriter::new(args.dump_dir, args.run_id);
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

/// Writes the app's minidumps, `<run>.dmp` and then `<run>.<n>.dmp`, each through its
/// `.partial` file.
///
/// It keeps serving after a dump: a panic the app recovered from asks for one too, and the run
/// goes on. The lifeline ends the monitor once the app is gone.
struct DumpWriter {
    dump_dir: PathBuf,
    run_id: String,
    /// Dumps started so far, which numbers the next one.
    started: AtomicU32,
    /// The final name of the dump being written; the server writes one at a time.
    writing: Mutex<Option<String>>,
}

impl DumpWriter {
    fn new(dump_dir: PathBuf, run_id: String) -> Self {
        Self { dump_dir, run_id, started: AtomicU32::new(0), writing: Mutex::new(None) }
    }

    fn lock_writing(&self) -> MutexGuard<'_, Option<String>> {
        self.writing.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    fn finish(&self, dump: MinidumpBinary, final_name: &str) -> io::Result<()> {
        dump.file.sync_all()?;
        // The partial's lock goes with `dump.file`, after the rename.
        fs::rename(&dump.path, self.dump_dir.join(final_name))
    }
}

impl ServerHandler for DumpWriter {
    fn create_minidump_file(&self) -> Result<(File, PathBuf), io::Error> {
        let seq = self.started.fetch_add(1, Ordering::Relaxed);
        let name = report::minidump_file_name(&self.run_id, seq);
        let path = self.dump_dir.join(report::partial_name(&name));
        // Locked until `finish` has renamed it and the dump's file is dropped, so another
        // process's startup cleanup leaves it alone (`report::cleanup_stale_partials`).
        let file = report::create_partial(&path)?;
        *self.lock_writing() = Some(name);
        Ok((file, path))
    }

    fn on_minidump_created(&self, result: Result<MinidumpBinary, minidumper::Error>) -> LoopAction {
        if let Some(name) = self.lock_writing().take()
            && let Err(err) = result.map_err(io::Error::other).and_then(|dump| self.finish(dump, &name))
        {
            eprintln!("sapphire_crash monitor: failed to write the minidump: {err}");
            let _ = fs::remove_file(self.dump_dir.join(report::partial_name(&name)));
        }
        // A dump doesn't mean the app is gone: a recovered panic asks for one and the run goes
        // on, so keep serving until the lifeline closes.
        LoopAction::Continue
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
    fn backoff_doubles_up_to_the_cap_and_gives_up_at_the_timeout() {
        let mut backoff = Backoff::new(Duration::from_millis(5));
        assert!(backoff.sleep() && backoff.sleep() && backoff.sleep(), "1, 2 and 4 ms");
        assert!(!backoff.sleep(), "7 ms slept is past the 5 ms timeout");

        let mut backoff = Backoff { delay: MAX_POLL_DELAY / 2, ..Backoff::new(Duration::from_secs(1)) };
        assert!(backoff.sleep());
        assert!(backoff.sleep());
        assert_eq!(backoff.delay, MAX_POLL_DELAY, "capped");
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
