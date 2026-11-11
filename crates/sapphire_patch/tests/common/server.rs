//! A static host on localhost that misbehaves on cue: HTTP/1.1 GETs with `Range`, kept-alive connections,
//! and optionally TLS. Each request is logged with the body bytes it was sent, so a test can count what
//! crossed the wire.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

/// What the host does with one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// Serves it as asked.
    None,
    /// Announces the whole body, sends half and closes the connection.
    Drop,
    /// Announces and sends half the body, cleanly.
    Truncate,
    /// Sends the body with one byte flipped.
    Corrupt,
    /// Answers a range request with the whole file, `200`.
    IgnoreRange,
    /// Announces and sends a kilobyte past what was asked.
    Extra,
    /// Sends half the body, then nothing for longer than any test's read timeout, then closes.
    Stall,
    /// Answers `404`.
    Missing,
    /// Waits this many milliseconds, then serves it as asked.
    Delay(u64),
    /// Announces the whole body and sends it a byte every [`DRIBBLE`], so each read arrives well inside any
    /// read timeout and the body never seems to stall.
    Dribble,
    /// Answers `302 Found`, sending the client to the same path on this port of 127.0.0.1, over plain HTTP.
    Redirect(u16),
}

/// A request as the host read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// Its number, from 0, across the host's life.
    pub index: usize,
    pub path: String,
    pub range: Option<Range<u64>>,
}

/// A request as the host answered it.
#[derive(Clone, Debug)]
pub struct Served {
    pub asked: Asked,
    pub fault: Fault,
    /// Body bytes written to the connection.
    pub sent: u64,
}

type Faults = Box<dyn FnMut(&Asked) -> Fault + Send>;

struct State {
    files: Mutex<BTreeMap<String, Arc<[u8]>>>,
    faults: Mutex<Faults>,
    log: Mutex<Vec<Served>>,
    asked: AtomicUsize,
    stopped: AtomicBool,
    tls: Option<Arc<rustls::ServerConfig>>,
}

/// The host, serving until dropped.
pub struct Host {
    addr: SocketAddr,
    state: Arc<State>,
}

/// How long [`Fault::Stall`] stalls: past every test's read timeout.
pub const STALL: Duration = Duration::from_secs(2);

/// How long [`Fault::Dribble`] waits between bytes: a manifest of 600 bytes takes 12 seconds.
pub const DRIBBLE: Duration = Duration::from_millis(20);

impl Host {
    /// Serves `files` by path on a port of its own, over plain HTTP.
    pub fn start(files: BTreeMap<String, Arc<[u8]>>) -> Self {
        Self::listen(files, None)
    }

    /// Serves `files` over TLS with `config`'s certificate.
    pub fn start_tls(files: BTreeMap<String, Arc<[u8]>>, config: rustls::ServerConfig) -> Self {
        Self::listen(files, Some(Arc::new(config)))
    }

    fn listen(files: BTreeMap<String, Arc<[u8]>>, tls: Option<Arc<rustls::ServerConfig>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a localhost port");
        let addr = listener.local_addr().expect("its address");
        let state = Arc::new(State {
            files: Mutex::new(files),
            faults: Mutex::new(Box::new(|_| Fault::None)),
            log: Mutex::new(Vec::new()),
            asked: AtomicUsize::new(0),
            stopped: AtomicBool::new(false),
            tls,
        });
        let accepting = state.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                if accepting.stopped.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(stream) = stream else { continue };
                let state = accepting.clone();
                thread::spawn(move || connection(stream, &state));
            }
        });
        Self { addr, state }
    }

    /// The port it listens on, on 127.0.0.1.
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// The release's base URL.
    pub fn url(&self) -> String {
        let scheme = if self.state.tls.is_some() { "https" } else { "http" };
        format!("{scheme}://127.0.0.1:{}/", self.addr.port())
    }

    /// Replaces what it serves.
    pub fn set_files(&self, files: BTreeMap<String, Arc<[u8]>>) {
        *self.state.files.lock().unwrap_or_else(PoisonError::into_inner) = files;
    }

    /// Decides each request's fault from now on.
    pub fn set_faults(&self, faults: impl FnMut(&Asked) -> Fault + Send + 'static) {
        *self.state.faults.lock().unwrap_or_else(PoisonError::into_inner) = Box::new(faults);
    }

    /// Every request answered so far, once the answers in flight are logged. A request is logged after its
    /// answer is written, so a client can read a whole answer, act on it and return before its connection's
    /// thread logs it: a Windows run saw an empty log after a refused manifest. It waits at most about
    /// 5000 sleeps of a millisecond, past a stall, counted rather than timed (R2: tests read no clock).
    pub fn log(&self) -> Vec<Served> {
        for _ in 0..5000 {
            let log = self.state.log.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if log.len() >= self.state.asked.load(Ordering::Acquire) {
                return log;
            }
            thread::sleep(Duration::from_millis(1));
        }
        self.log_so_far()
    }

    /// What's logged this moment, without waiting for answers in flight: for a test that polls the log
    /// while a download runs, to act between one request and the next.
    pub fn log_so_far(&self) -> Vec<Served> {
        self.state.log.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Body bytes sent so far.
    pub fn sent(&self) -> u64 {
        self.log().iter().map(|served| served.sent).sum()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::Relaxed);
        // Wakes the accepting thread, so it sees the stop.
        let _ = TcpStream::connect(self.addr);
    }
}

fn connection(stream: TcpStream, state: &State) {
    let _ = stream.set_nodelay(true);
    match &state.tls {
        None => serve(stream, state),
        Some(config) => {
            let Ok(server) = rustls::ServerConnection::new(config.clone()) else { return };
            serve(rustls::StreamOwned::new(server, stream), state);
        }
    }
}

/// Answers requests on one connection until it closes or a fault closes it.
fn serve(mut stream: impl Read + Write, state: &State) {
    let mut pending = Vec::new();
    loop {
        let Some(head) = read_head(&mut stream, &mut pending) else { return };
        let Some((path, range)) = parse(&head) else { return };
        let asked = Asked { index: state.asked.fetch_add(1, Ordering::Relaxed), path, range };
        let fault = (state.faults.lock().unwrap_or_else(PoisonError::into_inner))(&asked);
        let file = state.files.lock().unwrap_or_else(PoisonError::into_inner).get(&asked.path).cloned();
        let mut sent = 0;
        let keep_open = answer(&mut stream, file.as_deref(), &asked, fault, &mut sent).unwrap_or(false);
        state.log.lock().unwrap_or_else(PoisonError::into_inner).push(Served { asked, fault, sent });
        if !keep_open {
            return;
        }
    }
}

/// Reads one request's head, up to its blank line; requests have no body.
fn read_head(stream: &mut impl Read, pending: &mut Vec<u8>) -> Option<String> {
    loop {
        if let Some(end) = pending.windows(4).position(|window| window == b"\r\n\r\n") {
            let head = String::from_utf8(pending[..end].to_vec()).ok()?;
            pending.drain(..end + 4);
            return Some(head);
        }
        let mut chunk = [0; 1024];
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(read) => pending.extend_from_slice(&chunk[..read]),
        }
    }
}

/// A GET's path, without its leading `/`, and the range it asks for.
fn parse(head: &str) -> Option<(String, Option<Range<u64>>)> {
    let mut lines = head.split("\r\n");
    let path = lines.next()?.strip_prefix("GET /")?.split(' ').next()?.to_owned();
    let range = lines.find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.eq_ignore_ascii_case("range") {
            return None;
        }
        let (start, end) = value.trim().strip_prefix("bytes=")?.split_once('-')?;
        Some(start.parse::<u64>().ok()?..end.parse::<u64>().ok()? + 1)
    });
    Some((path, range))
}

/// Writes the answer; returns whether the connection stays open.
fn answer(
    stream: &mut impl Write,
    file: Option<&[u8]>,
    asked: &Asked,
    fault: Fault,
    sent: &mut u64,
) -> io::Result<bool> {
    if let Fault::Delay(millis) = fault {
        thread::sleep(Duration::from_millis(millis));
    }
    if let Fault::Redirect(port) = fault {
        let location = format!("http://127.0.0.1:{port}/{}", asked.path);
        stream.write_all(
            format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n").as_bytes(),
        )?;
        stream.flush()?;
        return Ok(true);
    }
    let Some(file) = file.filter(|_| fault != Fault::Missing) else {
        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")?;
        stream.flush()?;
        return Ok(true);
    };
    let len = file.len() as u64;
    let (status, mut body) = match (&asked.range, fault) {
        (Some(range), fault) if fault != Fault::IgnoreRange => {
            if range.start >= len || range.end <= range.start {
                stream.write_all(b"HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\n\r\n")?;
                return Ok(true);
            }
            let end = range.end.min(len);
            ("206 Partial Content", file[range.start as usize..end as usize].to_vec())
        }
        _ => ("200 OK", file.to_vec()),
    };
    let half = body.len() / 2;
    match fault {
        Fault::Truncate => body.truncate(half),
        Fault::Corrupt => body[half] ^= 0x5a,
        Fault::Extra => body.extend_from_slice(&[0xee; 1024]),
        _ => {}
    }
    let announced = body.len();
    stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {announced}\r\n\r\n").as_bytes())?;
    if fault == Fault::Dribble {
        for byte in body.chunks(1) {
            stream.write_all(byte)?;
            stream.flush()?;
            *sent += 1;
            thread::sleep(DRIBBLE);
        }
        return Ok(true);
    }
    let now = match fault {
        Fault::Drop | Fault::Stall => &body[..half],
        _ => &body[..],
    };
    stream.write_all(now)?;
    stream.flush()?;
    *sent += now.len() as u64;
    match fault {
        Fault::Drop => Ok(false),
        Fault::Stall => {
            thread::sleep(STALL);
            Ok(false)
        }
        _ => Ok(true),
    }
}
