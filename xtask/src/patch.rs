//! `cargo xtask patch`: a release served from this machine, and Exit test 2's lane (M4 P4).
//!
//! `patch serve <dir> [--port <n>]` serves a release directory as `cargo xtask release` lays it out, on
//! 127.0.0.1 only: HTTP/1.1 `GET` and `HEAD`, one byte range a request (`206`), connections kept alive, and a
//! line on stderr for each request with the bytes it sent and the run's total. Standard library only, so xtask
//! gains no dependency. The Simulator reaches it at `http://127.0.0.1:<port>/`; the emulator through
//! `adb reverse tcp:<port> tcp:<port>`. A dev build of the game points at it with `--update-host`.
//!
//! `patch rehearse [--out <dir>]` is Exit test 2's lane for one edit (a screen's text size): it cooks release 1
//! and, from a copy of `content/` with the edit, release 2 as a patch of it, both signed with the test seed
//! (`sapphire_patch::TEST_KEY`'s), and serves release 2. Then the game, a `dev` build with no window
//! (`--capture`), installed with release 1:
//!
//! - checks, downloads and installs in its boot, and draws release 2's title, byte for byte what a game
//!   shipped with release 2 draws; the bytes it fetched are counted against P0's 3,617 for the same edit;
//! - launched again, stands on release 2 and fetches only the host's manifest;
//! - offered a release 2 signed by a key it doesn't trust, starts on release 1 and installs nothing;
//! - killed while a block it asked for stalls, fetches at the next launch only what wasn't staged whole (no
//!   pack's header or table again), and stands on release 2.
//!
//! What it measured goes to `<out>/report.txt` (default `target/patch-rehearsal/`).

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use anyhow::{Context as _, bail, ensure};

use crate::cli;
use crate::content::{self, Cook};
use crate::release::{self, Release};
use crate::workspace::{self, BuildProfile, CargoBuild};

/// The test key's seed, `[0x50; 32]`, in hex: what `SAPPHIRE_RELEASE_KEY` holds to sign as the test key.
pub const TEST_SEED: &str = "5050505050505050505050505050505050505050505050505050505050505050";

/// The test key's public half, `sapphire_patch::TEST_KEY`, in hex: what no package may carry.
pub const TEST_KEY: &str = "3ef08959033028c8fc6fc6d491a12548e78df6aefb2d7c2c190c3f78cef3f50b";

/// A seed no build trusts, for the refused release.
const UNTRUSTED_SEED: &str = "5151515151515151515151515151515151515151515151515151515151515151";

/// What the same edit cost in P0's spike: the screen's blocks, its pack's tables and the manifest.
const P0_SCREEN_EDIT: u64 = 3_617;

/// The rehearsal's edit: the title's tagline from 18 points to 20.
const EDITED: &str = "ui/title.ui.ron";
const EDIT_FROM: &str = r#"(name: "tagline", role: "body", size: 18,"#;
const EDIT_TO: &str = r#"(name: "tagline", role: "body", size: 20,"#;

/// The longest any one run of the game may take before the lane fails.
const RUN_LIMIT: Duration = Duration::from_secs(600);
/// How often a wait on the game looks again. Waits count their looks, since xtask reads no clock (R2).
const POLL: Duration = Duration::from_millis(50);
/// The looks [`RUN_LIMIT`] allows.
const MOST_POLLS: u128 = RUN_LIMIT.as_millis() / POLL.as_millis();

/// `cargo xtask patch <serve|rehearse> ...`.
pub fn run(args: Vec<String>) -> anyhow::Result<()> {
    let mut args = cli::Args::new(args);
    match args.command().as_deref() {
        Some("serve") => {
            let (dir, port) = serve_args(args)?;
            ensure!(
                dir.join(release::MANIFEST).is_file(),
                "{} has no {}: lay a release out with `cargo xtask release`",
                dir.display(),
                release::MANIFEST
            );
            let host = Host::start(&dir, port, None, true)?;
            eprintln!("serving {} at http://127.0.0.1:{}/ (Ctrl-C stops it)", dir.display(), host.port());
            eprintln!("the emulator reaches it after `adb reverse tcp:{0} tcp:{0}`", host.port());
            loop {
                thread::park();
            }
        }
        Some("rehearse") => {
            let out = args
                .value::<PathBuf>("--out")?
                .unwrap_or_else(|| workspace::output_dir().join("patch-rehearsal"));
            args.finish(false)?;
            rehearse(&out)
        }
        other => bail!("patch takes `serve <dir> [--port <n>]` or `rehearse [--out <dir>]`, not {other:?}"),
    }
}

/// `serve`'s directory and port (8475 unless `--port` says).
fn serve_args(mut args: cli::Args) -> anyhow::Result<(PathBuf, u16)> {
    let port = args.value::<u16>("--port")?.unwrap_or(8475);
    // What's left once the port is taken: the directory, first.
    let dir = PathBuf::from(args.command().context("patch serve takes a release directory")?);
    args.finish(false)?;
    Ok((dir, port))
}

/// Fails if `bytes` hold the test key's public half: what a package's binary, or a release manifest it ships,
/// must never hold (m4-plan §3 "Keys"). `what` names it in the error.
pub fn refuse_test_key(what: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let key = hex(TEST_KEY);
    ensure!(
        !bytes.windows(key.len()).any(|window| window == key),
        "{what} trusts the test key, whose seed is public, so it can't ship: sign releases with the maintainer's \
         keys, and list only theirs in platform/patch.toml"
    );
    Ok(())
}

/// 32 bytes from 64 hex digits.
fn hex(digits: &str) -> [u8; 32] {
    std::array::from_fn(|at| u8::from_str_radix(&digits[at * 2..at * 2 + 2], 16).expect("hex"))
}

/// One request the host answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    /// The path asked for, from the release's root, with no leading `/`.
    pub path: String,
    /// The bytes asked for, first and last, if a range was.
    pub range: Option<(u64, u64)>,
    pub status: u16,
    /// Body bytes sent.
    pub bytes: u64,
}

/// What may hold a request unanswered, for a kill mid-download: `true` stalls it until the host stops.
pub type Stall = Arc<dyn Fn(&str, Option<(u64, u64)>) -> bool + Send + Sync>;

/// A static host for a release directory on 127.0.0.1 (see the [module docs](self)). Stops when dropped.
pub struct Host {
    port: u16,
    log: Arc<Mutex<Vec<Served>>>,
    stalled: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Host {
    /// Serves `dir` on `port` (0 picks a free one), holding what `stall` says unanswered, and printing each
    /// request if `echo`.
    pub fn start(dir: &Path, port: u16, stall: Option<Stall>, echo: bool) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port)))
            .with_context(|| format!("failed to listen on 127.0.0.1:{port}"))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let log = Arc::new(Mutex::new(Vec::new()));
        let stalled = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Shared {
            dir: dir.to_owned(),
            log: log.clone(),
            stalled: stalled.clone(),
            stop: stop.clone(),
            stall,
            echo,
            total: Mutex::new(0),
        });
        let accept = thread::Builder::new().name("patch-host".to_owned()).spawn(move || {
            while !shared.stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let shared = shared.clone();
                        let _ = thread::Builder::new().name("patch-host-conn".to_owned()).spawn(move || {
                            if let Err(error) = shared.connection(stream)
                                && shared.echo
                            {
                                eprintln!("patch host: {error}");
                            }
                        });
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => eprintln!("patch host: {error}"),
                }
            }
        })?;
        Ok(Self { port, log, stalled, stop, accept: Some(accept) })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// `http://127.0.0.1:<port>/`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// What it has answered since the last call.
    pub fn take_log(&self) -> Vec<Served> {
        std::mem::take(&mut *self.log.lock().expect("the log"))
    }

    /// Requests it holds unanswered.
    pub fn stalled(&self) -> usize {
        self.stalled.load(Ordering::Acquire)
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

/// What a host's threads share.
struct Shared {
    dir: PathBuf,
    log: Arc<Mutex<Vec<Served>>>,
    stalled: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    stall: Option<Stall>,
    echo: bool,
    total: Mutex<u64>,
}

/// The most a request's line and headers may take.
const MOST_HEADERS: usize = 16 << 10;

impl Shared {
    /// Answers requests on `stream` until it closes, asks to, errs or the host stops.
    fn connection(&self, mut stream: TcpStream) -> io::Result<()> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        let mut buffer = Vec::new();
        loop {
            let Some(end) = self.read_head(&mut stream, &mut buffer)? else { return Ok(()) };
            let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
            buffer.drain(..end + 4);
            let request = parse(&head);
            let keep = request.as_ref().is_ok_and(|request| request.keep_alive);
            self.answer(&mut stream, request)?;
            if !keep || self.stop.load(Ordering::Acquire) {
                let _ = stream.shutdown(Shutdown::Both);
                return Ok(());
            }
        }
    }

    /// Reads into `buffer` until it holds a whole head; returns where its blank line starts, or `None` once the
    /// peer closes or the host stops.
    fn read_head(&self, stream: &mut TcpStream, buffer: &mut Vec<u8>) -> io::Result<Option<usize>> {
        let mut chunk = [0; 4096];
        loop {
            if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
                return Ok(Some(end));
            }
            if buffer.len() > MOST_HEADERS {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "a request's head is too long"));
            }
            match stream.read(&mut chunk) {
                Ok(0) => return Ok(None),
                Ok(read) => buffer.extend_from_slice(&chunk[..read]),
                Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
                    if self.stop.load(Ordering::Acquire) {
                        return Ok(None);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Answers one request.
    fn answer(&self, stream: &mut TcpStream, request: Result<Request, u16>) -> io::Result<()> {
        let request = match request {
            Ok(request) => request,
            Err(status) => return self.status_only(stream, "", None, status, &[]),
        };
        let Some(file) = self.file(&request.path) else {
            return self.status_only(stream, &request.path, request.range, 404, &[]);
        };
        let mut file = File::open(&file)?;
        let len = file.metadata()?.len();
        let (status, first, count) = match request.range {
            None => (200, 0, len),
            Some((first, last)) if first < len && first <= last => {
                (206, first, last.min(len - 1) - first + 1)
            }
            Some(_) => {
                let range = format!("Content-Range: bytes */{len}\r\n");
                return self.status_only(stream, &request.path, request.range, 416, range.as_bytes());
            }
        };
        if let Some(stall) = &self.stall
            && stall(&request.path, request.range)
        {
            self.stalled.fetch_add(1, Ordering::AcqRel);
            if self.echo {
                eprintln!("{} {:?}: stalled", request.path, request.range);
            }
            while !self.stop.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(20));
            }
            return Ok(());
        }
        let mut head = format!(
            "HTTP/1.1 {status} {}\r\nContent-Length: {count}\r\nContent-Type: application/octet-stream\r\n\
             Accept-Ranges: bytes\r\n",
            reason(status)
        );
        if status == 206 {
            let _ = write!(head, "Content-Range: bytes {first}-{}/{len}\r\n", first + count - 1);
        }
        head.push_str(if request.keep_alive {
            "Connection: keep-alive\r\n\r\n"
        } else {
            "Connection: close\r\n\r\n"
        });
        stream.write_all(head.as_bytes())?;
        let sent = if request.head {
            0
        } else {
            file.seek(SeekFrom::Start(first))?;
            io::copy(&mut file.take(count), stream)?
        };
        stream.flush()?;
        self.logged(Served { path: request.path, range: request.range, status, bytes: sent });
        Ok(())
    }

    /// A reply with no body.
    fn status_only(
        &self,
        stream: &mut TcpStream,
        path: &str,
        range: Option<(u64, u64)>,
        status: u16,
        extra: &[u8],
    ) -> io::Result<()> {
        let head = format!("HTTP/1.1 {status} {}\r\nContent-Length: 0\r\n", reason(status));
        stream.write_all(head.as_bytes())?;
        stream.write_all(extra)?;
        stream.write_all(b"\r\n")?;
        stream.flush()?;
        self.logged(Served { path: path.to_owned(), range, status, bytes: 0 });
        Ok(())
    }

    fn logged(&self, served: Served) {
        let total = {
            let mut total = self.total.lock().expect("the total");
            *total += served.bytes;
            *total
        };
        if self.echo {
            let range =
                served.range.map(|(first, last)| format!(" bytes={first}-{last}")).unwrap_or_default();
            eprintln!("GET /{}{range} {} {} bytes (total {total})", served.path, served.status, served.bytes);
        }
        self.log.lock().expect("the log").push(served);
    }

    /// The file `path` names under the release, if it's one: never outside it.
    fn file(&self, path: &str) -> Option<PathBuf> {
        let relative = Path::new(path);
        let plain = !path.is_empty()
            && !path.contains('\\')
            && relative.components().all(|component| matches!(component, Component::Normal(_)));
        let file = self.dir.join(relative);
        (plain && file.is_file()).then_some(file)
    }
}

/// A request's line and the headers that matter.
#[derive(Debug, PartialEq, Eq)]
struct Request {
    path: String,
    head: bool,
    range: Option<(u64, u64)>,
    keep_alive: bool,
}

/// Reads a request's head, or the status that refuses it.
fn parse(head: &str) -> Result<Request, u16> {
    let mut lines = head.split("\r\n");
    let mut line = lines.next().unwrap_or_default().split(' ');
    let (method, target, version) = (line.next(), line.next(), line.next());
    let head = match method {
        Some("GET") => false,
        Some("HEAD") => true,
        _ => return Err(405),
    };
    let path = target.and_then(|target| target.strip_prefix('/')).ok_or(400u16)?;
    let path = path.split('?').next().unwrap_or_default().to_owned();
    let mut keep_alive = version == Some("HTTP/1.1");
    let mut range = None;
    for header in lines {
        let Some((name, value)) = header.split_once(':') else { continue };
        let value = value.trim();
        if name.eq_ignore_ascii_case("connection") {
            keep_alive = !value.eq_ignore_ascii_case("close");
        } else if name.eq_ignore_ascii_case("range") {
            let (first, last) =
                value.strip_prefix("bytes=").and_then(|range| range.split_once('-')).ok_or(416u16)?;
            let first = first.parse().map_err(|_| 416u16)?;
            let last = if last.is_empty() { u64::MAX } else { last.parse().map_err(|_| 416u16)? };
            range = Some((first, last));
        }
    }
    Ok(Request { path, head, range, keep_alive })
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        _ => "",
    }
}

/// What the rehearsal made, by path under its directory.
struct Rehearsal {
    out: PathBuf,
    game: PathBuf,
    report: String,
}

/// Exit test 2's lane (see the [module docs](self)).
fn rehearse(out: &Path) -> anyhow::Result<()> {
    workspace::remove_dir_if_exists(out)?;
    fs::create_dir_all(out.join("logs"))?;
    let game = workspace::build_binary(
        &CargoBuild {
            package: "game_app",
            target: None,
            profile: BuildProfile::Tools,
            features: &["dev".to_owned()],
            env: &[],
        },
        "game_app",
    )?;
    let mut rehearsal = Rehearsal { out: out.to_owned(), game, report: String::new() };
    let r = &mut rehearsal;

    // Release 1, as the workspace cooks it, and the game installed with it.
    let cooked = content::cook(Cook::Desktop)?;
    let cook1 = out.join("cook1");
    workspace::copy_dir(&cooked.dir, &cook1)?;
    drop(cooked);
    let host_dir = out.join("host");
    let untrusted_dir = out.join("host-untrusted");
    let first = release(1, None);
    release::publish_with_test_seeds(&cook1, &host_dir, &first, TEST_SEED)?;
    release::publish_with_test_seeds(&cook1, &untrusted_dir, &first, UNTRUSTED_SEED)?;
    let app1 = installed(&cook1, &host_dir.join("manifests/1.smanifest"), &out.join("app1"))?;

    // Release 2: the same content with one edit, as a patch of release 1.
    let source = out.join("source2");
    workspace::copy_dir(&workspace::root().join(content::SOURCE_DIR), &source)?;
    let edited = source.join(EDITED);
    let text = fs::read_to_string(&edited)?;
    ensure!(
        text.matches(EDIT_FROM).count() == 1,
        "{EDITED} no longer has the tagline's size this lane edits"
    );
    fs::write(&edited, text.replace(EDIT_FROM, EDIT_TO))?;
    let cook2 = out.join("cook2");
    content::cook_copy(&source, &out.join("loose2"), &cook2, Cook::Desktop)?;
    let second = release(2, Some(host_dir.join("manifests/1.smanifest")));
    release::publish_with_test_seeds(&cook2, &host_dir, &second, TEST_SEED)?;
    let untrusted = release(2, Some(untrusted_dir.join("manifests/1.smanifest")));
    release::publish_with_test_seeds(&cook2, &untrusted_dir, &untrusted, UNTRUSTED_SEED)?;
    let app2 = installed(&cook2, &host_dir.join("manifests/2.smanifest"), &out.join("app2"))?;

    // What each release draws, shipped as it is.
    let shipped1 = r.capture("shipped-1", &app1, None, None)?;
    let shipped2 = r.capture("shipped-2", &app2, None, None)?;
    ensure!(shipped1 != shipped2, "the edit doesn't show in the capture: the lane would prove nothing");

    // The patch, in the boot.
    let host = Host::start(&host_dir, 0, None, false)?;
    let patch_dir = out.join("patch");
    let patched = r.capture("patched", &app1, Some(&patch_dir), Some(&host.url()))?;
    let fetched = host.take_log();
    ensure!(patched == shipped2, "the patched boot didn't draw release 2's title");
    let installed_manifest =
        fs::read(patch_dir.join("installed").join(release::MANIFEST)).context("nothing was installed")?;
    ensure!(
        installed_manifest == fs::read(host_dir.join("manifests/2.smanifest"))?,
        "release 2 isn't installed"
    );
    let total: u64 = fetched.iter().map(|served| served.bytes).sum();
    let tables = header_and_tables(&fetched);
    let _ = writeln!(r.report, "patch of one screen edit ({EDITED}: tagline size 18 to 20), release 1 to 2:");
    for served in &fetched {
        let kind = if served.path == release::MANIFEST {
            "manifest"
        } else if tables.contains(&(served.path.clone(), served.range)) {
            "header or table"
        } else if served.path.starts_with("files/") {
            "file"
        } else {
            "blocks"
        };
        let _ =
            writeln!(r.report, "  {kind:16} {:>8} bytes  {} {:?}", served.bytes, served.path, served.range);
    }
    let _ = writeln!(
        r.report,
        "  total {total} bytes in {} requests (P0 measured {P0_SCREEN_EDIT})",
        fetched.len()
    );
    ensure!(
        fetched.iter().all(|served| matches!(served.status, 200 | 206)),
        "the host refused a request: {fetched:?}"
    );

    // Launched again: release 2 stands, and only the manifest crosses.
    let again = r.capture("relaunched", &app1, Some(&patch_dir), Some(&host.url()))?;
    let checked = host.take_log();
    ensure!(again == shipped2, "the relaunch didn't stand on release 2");
    ensure!(
        checked.iter().all(|served| served.path == release::MANIFEST),
        "the relaunch fetched more than the manifest: {checked:?}"
    );
    let bytes: u64 = checked.iter().map(|served| served.bytes).sum();
    let _ = writeln!(r.report, "relaunch: release 2 mounted, {bytes} bytes fetched (the manifest)");
    drop(host);

    // A release signed by a key the build doesn't trust.
    let host = Host::start(&untrusted_dir, 0, None, false)?;
    let refused_dir = out.join("patch-refused");
    let refused = r.capture("refused", &app1, Some(&refused_dir), Some(&host.url()))?;
    let asked = host.take_log();
    ensure!(refused == shipped1, "a refused release changed what the game drew");
    ensure!(
        asked.iter().all(|served| served.path == release::MANIFEST),
        "a refused release's packs were fetched: {asked:?}"
    );
    ensure!(
        !refused_dir.join("installed").join(release::MANIFEST).exists(),
        "a refused release was installed"
    );
    let log = fs::read_to_string(out.join("logs/refused.log"))?;
    let why = log.lines().find(|line| line.contains("no update")).unwrap_or("(no reason logged)");
    let _ =
        writeln!(r.report, "untrusted release 2: refused, release 1 drawn; the game said: {}", why.trim());
    drop(host);

    // Killed mid-download, then resumed.
    let blocks: BTreeSet<_> = fetched
        .iter()
        .filter(|served| {
            served.path.starts_with("packs/") && !tables.contains(&(served.path.clone(), served.range))
        })
        .map(|served| (served.path.clone(), served.range))
        .collect();
    ensure!(!blocks.is_empty(), "the patch fetched no blocks to stall");
    let stalled_blocks = blocks.clone();
    let stall: Stall = Arc::new(move |path, range| stalled_blocks.contains(&(path.to_owned(), range)));
    let host = Host::start(&host_dir, 0, Some(stall), false)?;
    let killed_dir = out.join("patch-killed");
    let mut child = r.spawn("killed", &app1, Some(&killed_dir), Some(&host.url()))?;
    let mut polls = 0;
    while host.stalled() == 0 {
        if let Some(status) = child.try_wait()? {
            bail!("the game ended ({status}) before it asked for a block");
        }
        polls += 1;
        ensure!(polls < MOST_POLLS, "the game never asked for a block");
        thread::sleep(POLL);
    }
    child.kill()?;
    child.wait()?;
    let before_kill = host.take_log();
    drop(host);
    let staged_tables = fs::read_dir(killed_dir.join("staging/tables")).map(Iterator::count).unwrap_or(0);
    ensure!(staged_tables > 0, "the kill came before any table was staged");
    let host = Host::start(&host_dir, 0, None, false)?;
    let resumed = r.capture("resumed", &app1, Some(&killed_dir), Some(&host.url()))?;
    let after = host.take_log();
    ensure!(resumed == shipped2, "the resumed patch didn't stand on release 2");
    ensure!(
        !after.iter().any(|served| tables.contains(&(served.path.clone(), served.range))),
        "the resumed download fetched a header or table again: {after:?}"
    );
    let (killed_bytes, resumed_bytes): (u64, u64) =
        (before_kill.iter().map(|served| served.bytes).sum(), after.iter().map(|served| served.bytes).sum());
    let _ = writeln!(
        r.report,
        "killed at a stalled block after {killed_bytes} bytes ({staged_tables} tables staged); the next launch \
         fetched {resumed_bytes} bytes in {} requests, no header or table again, and stood on release 2",
        after.len()
    );

    let report = out.join("report.txt");
    fs::write(&report, &r.report)?;
    eprint!("{}", r.report);
    eprintln!("patch rehearsal passed; report in {}", report.display());
    Ok(())
}

/// A release that's current until 2099.
fn release(version: u64, patch_from: Option<PathBuf>) -> Release {
    Release { version, expires: "2099-01-01".to_owned(), min_build: 0, next_keys: None, patch_from }
}

/// The content an app ships: a copy of the cook in `cook` at `out`, with the signed `manifest` in place.
fn installed(cook: &Path, manifest: &Path, out: &Path) -> anyhow::Result<PathBuf> {
    workspace::copy_dir(cook, out)?;
    fs::copy(manifest, out.join(release::MANIFEST))?;
    Ok(out.to_owned())
}

/// Each pack's header and table requests in a download's log: the first two it asked of each pack.
fn header_and_tables(log: &[Served]) -> BTreeSet<(String, Option<(u64, u64)>)> {
    let mut seen = std::collections::BTreeMap::<&str, usize>::new();
    log.iter()
        .filter(|served| served.path.starts_with("packs/"))
        .filter(|served| {
            let count = seen.entry(&served.path).or_default();
            *count += 1;
            *count <= 2
        })
        .map(|served| (served.path.clone(), served.range))
        .collect()
}

impl Rehearsal {
    /// Starts the game on `content`, with no window, patching into `patch_dir` from `host` if given.
    fn spawn(
        &self,
        name: &str,
        content: &Path,
        patch_dir: Option<&Path>,
        host: Option<&str>,
    ) -> anyhow::Result<Child> {
        let png = self.out.join(format!("{name}.png"));
        let log = File::create(self.out.join("logs").join(format!("{name}.log")))?;
        let mut command = Command::new(&self.game);
        command
            .args(["--capture".as_ref(), png.as_os_str()])
            .args(["--size", "640x360", "--ticks", "60"])
            .env("SAPPHIRE_CONTENT_DIR", content)
            .env("RUST_LOG", "info")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some(dir) = patch_dir {
            command.args(["--patch-dir".as_ref(), dir.as_os_str()]);
        }
        if let Some(host) = host {
            command.args(["--update-host", host]);
        }
        eprintln!("$ {}", workspace::shell_line(&command));
        command.spawn().with_context(|| format!("failed to start {}", self.game.display()))
    }

    /// Runs the game as [`Self::spawn`] does to the end, and returns the frame it captured.
    fn capture(
        &self,
        name: &str,
        content: &Path,
        patch_dir: Option<&Path>,
        host: Option<&str>,
    ) -> anyhow::Result<Vec<u8>> {
        let mut child = self.spawn(name, content, patch_dir, host)?;
        let mut polls = 0;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            polls += 1;
            if polls > MOST_POLLS {
                child.kill()?;
                child.wait()?;
                bail!("the {name} run took longer than {RUN_LIMIT:?}");
            }
            thread::sleep(POLL);
        };
        let log = self.out.join("logs").join(format!("{name}.log"));
        ensure!(status.success(), "the {name} run failed ({status}): see {}", log.display());
        fs::read(self.out.join(format!("{name}.png")))
            .with_context(|| format!("the {name} run captured nothing"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// xtask doesn't link `sapphire_patch`, so it reads the key from its source.
    #[test]
    fn the_test_key_is_sapphire_patchs() {
        let source = fs::read_to_string(workspace::root().join("crates/sapphire_patch/src/keys.rs")).unwrap();
        let array =
            source.split("pub const TEST_KEY: PublicKey = [").nth(1).unwrap().split("];").next().unwrap();
        let bytes: Vec<u8> = array
            .split(',')
            .map(str::trim)
            .filter(|byte| !byte.is_empty())
            .map(|byte| u8::from_str_radix(byte.trim_start_matches("0x"), 16).unwrap())
            .collect();
        assert_eq!(bytes, hex(TEST_KEY));
        assert!(source.contains("[0x50; 32]"), "the seed is sapphire_patch's");
        assert_eq!(TEST_SEED, "50".repeat(32));
    }

    #[test]
    fn serve_takes_a_directory_and_a_port() {
        let parse = |line: &str| serve_args(cli::Args::new(line.split_whitespace().map(str::to_owned)));
        assert_eq!(parse("host --port 9000").unwrap(), (PathBuf::from("host"), 9000));
        assert_eq!(parse("--port=9000 host").unwrap(), (PathBuf::from("host"), 9000));
        assert_eq!(parse("host").unwrap(), (PathBuf::from("host"), 8475));
        assert!(parse("").is_err() && parse("host other").is_err() && parse("host --port many").is_err());
    }

    #[test]
    fn a_package_holding_the_test_key_is_refused() {
        let mut binary = vec![7; 100];
        refuse_test_key("the game", &binary).unwrap();
        binary.splice(40..40, hex(TEST_KEY));
        let refused = refuse_test_key("the game", &binary).unwrap_err().to_string();
        assert!(refused.contains("the game trusts the test key"), "{refused}");
    }

    #[test]
    fn requests_are_read_as_the_patcher_sends_them() {
        let request = parse("GET /packs/ab.spack HTTP/1.1\r\nHost: x\r\nRange: bytes=256-1023").unwrap();
        assert_eq!(
            request,
            Request {
                path: "packs/ab.spack".to_owned(),
                head: false,
                range: Some((256, 1023)),
                keep_alive: true
            }
        );
        let request = parse("HEAD /release.smanifest HTTP/1.1\r\nConnection: close").unwrap();
        assert!(request.head && !request.keep_alive && request.range.is_none());
        assert_eq!(parse("GET /a HTTP/1.1\r\nRange: bytes=9-").unwrap().range, Some((9, u64::MAX)));
        assert_eq!(parse("POST /a HTTP/1.1"), Err(405));
        assert_eq!(parse("GET /a HTTP/1.1\r\nRange: lines=1-2"), Err(416));
    }

    /// The host on a real socket: whole files, ranges, a range past the end, a path out of the release, and
    /// several requests on one connection.
    #[test]
    fn the_host_serves_a_release_and_its_ranges() {
        let dir = std::env::temp_dir().join(format!("xtask-patch-host-{}", std::process::id()));
        fs::create_dir_all(dir.join("packs")).unwrap();
        fs::write(dir.join("release.smanifest"), b"manifest").unwrap();
        fs::write(dir.join("packs/a.spack"), (0..=255u8).collect::<Vec<_>>()).unwrap();
        let host = Host::start(&dir, 0, None, false).unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", host.port())).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut ask = |request: &str| -> String {
            stream.write_all(request.as_bytes()).unwrap();
            let mut reply = Vec::new();
            let mut byte = [0];
            while !reply.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                reply.push(byte[0]);
            }
            let head = String::from_utf8(reply).unwrap();
            let len: usize =
                head.lines().find_map(|line| line.strip_prefix("Content-Length: ")).unwrap().parse().unwrap();
            let mut body = vec![0; len];
            stream.read_exact(&mut body).unwrap();
            format!(
                "{}|{}",
                head.lines().next().unwrap(),
                body.iter().map(|b| format!("{b:02x}")).collect::<String>()
            )
        };
        assert_eq!(
            ask("GET /release.smanifest HTTP/1.1\r\n\r\n"),
            format!("HTTP/1.1 200 OK|{}", "6d616e6966657374")
        );
        assert_eq!(
            ask("GET /packs/a.spack HTTP/1.1\r\nRange: bytes=16-19\r\n\r\n"),
            "HTTP/1.1 206 Partial Content|10111213"
        );
        assert_eq!(
            ask("GET /packs/a.spack HTTP/1.1\r\nRange: bytes=300-400\r\n\r\n"),
            "HTTP/1.1 416 Range Not Satisfiable|"
        );
        assert_eq!(ask("GET /../outside HTTP/1.1\r\n\r\n"), "HTTP/1.1 404 Not Found|");
        assert_eq!(ask("GET /packs HTTP/1.1\r\n\r\n"), "HTTP/1.1 404 Not Found|");
        let log = host.take_log();
        assert_eq!(log.len(), 5);
        assert_eq!(
            log[1],
            Served { path: "packs/a.spack".to_owned(), range: Some((16, 19)), status: 206, bytes: 4 }
        );
        drop(host);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_downloads_first_two_requests_of_a_pack_are_its_header_and_table() {
        let served = |path: &str, first| Served {
            path: path.to_owned(),
            range: Some((first, first + 1)),
            status: 206,
            bytes: 2,
        };
        let log = [
            Served { path: "release.smanifest".to_owned(), range: None, status: 200, bytes: 9 },
            served("packs/a.spack", 0),
            served("packs/a.spack", 256),
            served("packs/b.spack", 0),
            served("packs/b.spack", 256),
            served("packs/a.spack", 4096),
        ];
        let tables = header_and_tables(&log);
        assert_eq!(tables.len(), 4);
        assert!(!tables.contains(&("packs/a.spack".to_owned(), Some((4096, 4097)))));
    }
}
