//! The desktop half's connection: [`Client`], which pushes to a device and reads its state.

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use sapphire_assets::pack::{PackError, PackHeader};
use sapphire_time::Stopwatch;

use crate::protocol::{
    CHUNK_LEN, Capture, DeviceStatus, LinkError, PackOutcome, Preamble, Reply, ReplyBody, Request,
    RequestBody, SourceFile, read_frame, write_frame,
};

/// How long a device has to answer a request, and to connect; and how long it may go without
/// taking any of the bytes sent to it. Pushed packs get [`Client::PACK_TIMEOUT`] to answer.
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to a device's live link. Requests go one at a time: each waits for its reply.
///
/// # Deadlines
///
/// A request waits at most its timeout for its reply in all, however the device sends it (a
/// byte at a time, or behind replies to other requests). What it sends has a deadline on
/// progress instead: the device must keep taking it, so a large pack over a slow USB link takes
/// as long as it needs, while one that stopped reading (suspended, stalled) fails the send as
/// [`LinkError::Stalled`] once it has taken nothing for [`TIMEOUT`], or up to twice that as the
/// OS counts a send's timeout (from the last bytes a write took, or from the write's start).
///
/// A request that fails on the connection (it timed out or stalled, or the stream broke or
/// carried something malformed) closes it: where in the stream a reply stopped is unknown, so no
/// later frame could be found. Later requests fail with [`LinkError::Closed`]
/// ([`Self::is_closed`]); connect again for more. A request that timed out may still apply on
/// the device, later or already: documents, cvars, shaders and packs can be pushed again safely,
/// and the status lists the generation of every asset pushed.
#[derive(Debug)]
pub struct Client {
    reader: BufReader<DeadlineReader>,
    writer: BufWriter<TcpStream>,
    next_id: u32,
    /// How long the device may take none of the bytes sent: [`TIMEOUT`], shorter in tests.
    progress: Duration,
    /// A request failed on the connection, which is shut down.
    closed: bool,
}

impl Client {
    /// How long a pushed pack may take to be swapped in: its changed assets read, decoded and
    /// uploaded, within the device's per-frame upload budget.
    pub const PACK_TIMEOUT: Duration = Duration::from_secs(60);

    /// Connects to a device listening at `address`.
    pub fn connect(address: SocketAddr) -> Result<Self, LinkError> {
        Self::handshake(TcpStream::connect_timeout(&address, TIMEOUT)?)
    }

    /// Takes the next device that dials `listener` (behind `adb reverse`).
    pub fn accept(listener: &TcpListener) -> Result<Self, LinkError> {
        let (stream, _) = listener.accept()?;
        Self::handshake(stream)
    }

    fn handshake(stream: TcpStream) -> Result<Self, LinkError> {
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        (&stream).write_all(&Preamble::CURRENT.to_bytes())?;
        let mut reader =
            DeadlineReader { stream: stream.try_clone()?, deadline: Some((Stopwatch::start(), TIMEOUT)) };
        Preamble::read(&mut reader)?.check()?;
        reader.deadline = None;
        Ok(Self {
            reader: BufReader::new(reader),
            writer: BufWriter::new(stream),
            next_id: 0,
            progress: TIMEOUT,
            closed: false,
        })
    }

    /// Whether a request failed on the connection, which closed it: every request now fails
    /// with [`LinkError::Closed`].
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The device's state.
    pub fn status(&mut self) -> Result<DeviceStatus, LinkError> {
        match self.request(RequestBody::Status, TIMEOUT)? {
            ReplyBody::Status(status) => Ok(status),
            other => Err(unexpected("status", &other)),
        }
    }

    /// Sets cvar `name` from text, and returns its name and value as the device has them now.
    pub fn set_cvar(&mut self, name: &str, value: &str) -> Result<(String, String), LinkError> {
        let body = RequestBody::SetCvar { name: name.to_owned(), value: value.to_owned() };
        match self.request(body, TIMEOUT)? {
            ReplyBody::CvarSet { name, value } => Ok((name, value)),
            other => Err(unexpected("cvar set", &other)),
        }
    }

    /// Pushes a RON document, named `origin` in messages, and returns what took it.
    pub fn push_document(&mut self, origin: &str, text: String) -> Result<String, LinkError> {
        let body = RequestBody::Document { origin: origin.to_owned(), text };
        match self.request(body, TIMEOUT)? {
            ReplyBody::DocumentApplied { owner } => Ok(owner),
            other => Err(unexpected("document applied", &other)),
        }
    }

    /// Pushes shader sources, and returns the programs and pipelines the device built from them
    /// and the frames it drew meanwhile, once they show.
    pub fn push_shaders(&mut self, files: Vec<SourceFile>) -> Result<(u32, u32, u32), LinkError> {
        // Building pipelines takes the device frames, seconds when its driver compiles them cold.
        match self.request(RequestBody::Shaders { files }, Self::PACK_TIMEOUT)? {
            ReplyBody::ShadersApplied { programs, pipelines, frames } => Ok((programs, pipelines, frames)),
            other => Err(unexpected("shaders applied", &other)),
        }
    }

    /// The next frame the device draws, as a PNG, with everything applied before it.
    pub fn capture(&mut self) -> Result<Capture, LinkError> {
        // A phone reads the frame back and compresses it, which takes it a moment.
        match self.request(RequestBody::Capture, Self::PACK_TIMEOUT)? {
            ReplyBody::Captured(capture) => Ok(capture),
            other => Err(unexpected("captured", &other)),
        }
    }

    /// Pushes `bytes`, a whole pack, as pack `name`: only the ranges the device asks for cross,
    /// in chunks. Returns once the device swapped in what changed, with what it did.
    pub fn push_pack(&mut self, name: &str, bytes: &[u8]) -> Result<PackOutcome, LinkError> {
        let invalid = |error| LinkError::InvalidPack { name: name.to_owned(), error };
        let header = PackHeader::read(bytes).map_err(invalid)?;
        let head = bytes.get(..header.data_offset as usize).ok_or_else(|| {
            invalid(PackError::Truncated { expected: header.data_offset, found: bytes.len() as u64 })
        })?;
        let begin =
            RequestBody::PackBegin { name: name.to_owned(), len: bytes.len() as u64, head: head.to_vec() };
        let ranges = match self.request(begin, TIMEOUT)? {
            ReplyBody::PackNeed { ranges } => ranges,
            other => return Err(unexpected("pack need", &other)),
        };
        for range in ranges {
            let Some(asked) = usize::try_from(range.offset)
                .ok()
                .zip(usize::try_from(range.end()).ok())
                .and_then(|(start, end)| bytes.get(start..end))
            else {
                // The device holds a push begun that this one won't finish.
                return Err(self.close(LinkError::PastPack(name.to_owned())));
            };
            for (index, chunk) in asked.chunks(CHUNK_LEN).enumerate() {
                let offset = range.offset + (index * CHUNK_LEN) as u64;
                self.send(RequestBody::PackChunk { offset, bytes: chunk.to_vec() })?;
            }
        }
        match self.request(RequestBody::PackEnd, Self::PACK_TIMEOUT)? {
            ReplyBody::PackApplied(outcome) => Ok(outcome),
            other => Err(unexpected("pack applied", &other)),
        }
    }

    /// Sends `body` and waits up to `timeout` in all for its reply. An error reply is
    /// [`LinkError::Refused`].
    fn request(&mut self, body: RequestBody, timeout: Duration) -> Result<ReplyBody, LinkError> {
        let id = self.send(body)?;
        if let Err(error) = self.writer.flush() {
            return Err(self.sending_failed(error.into()));
        }
        self.reader.get_mut().deadline = Some((Stopwatch::start(), timeout));
        let reply = loop {
            match read_frame::<Reply>(&mut self.reader) {
                Ok(reply) if reply.id == id => break Ok(reply.body),
                // A reply to no request waiting (a device that answered one twice): its time
                // counts against this request's.
                Ok(_) => {}
                Err(LinkError::Io(error))
                    if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
                {
                    break Err(LinkError::TimedOut(timeout));
                }
                Err(error) => break Err(error),
            }
        };
        self.reader.get_mut().deadline = None;
        match reply {
            Ok(ReplyBody::Error(message)) => Err(LinkError::Refused(message)),
            Ok(body) => Ok(body),
            // Wherever in a frame the reading stopped, the next frame can't be found.
            Err(error) => Err(self.close(error)),
        }
    }

    /// Writes `body` as a request with the next ID, which it returns. A request too large for
    /// a frame is refused before any of it is written, which leaves the connection as it was.
    fn send(&mut self, body: RequestBody) -> Result<u32, LinkError> {
        if self.closed {
            return Err(LinkError::Closed);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        match write_frame(&mut self.writer, &Request { id, body }) {
            Ok(()) => Ok(id),
            Err(error @ LinkError::FrameTooLarge(_)) => Err(error),
            Err(error) => Err(self.sending_failed(error)),
        }
    }

    /// Closes the connection after a write failed partway through a frame: as
    /// [`LinkError::Stalled`] if the device took nothing for [`Self::progress`].
    fn sending_failed(&mut self, error: LinkError) -> LinkError {
        let error = match error {
            LinkError::Io(error)
                if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
            {
                LinkError::Stalled(self.progress)
            }
            error => error,
        };
        self.close(error)
    }

    /// Shuts the connection down after `error` left it out of step, and returns the error.
    fn close(&mut self, error: LinkError) -> LinkError {
        self.closed = true;
        let _ = self.writer.get_ref().shutdown(Shutdown::Both);
        error
    }

    /// Gives the device `progress` without taking any bytes before a send fails (tests).
    #[cfg(test)]
    fn set_progress(&mut self, progress: Duration) {
        self.progress = progress;
        self.writer.get_ref().set_write_timeout(Some(progress)).expect("a write timeout");
    }
}

fn unexpected(expected: &'static str, found: &ReplyBody) -> LinkError {
    LinkError::Unexpected { expected, found: found.name() }
}

/// The connection's reading side, which keeps a request to its deadline in all: before each read
/// from the socket, the socket's timeout is set to the time the request has left. A timeout per
/// read alone lets a device that drips its reply, a byte within each, take as long as it likes.
#[derive(Debug)]
struct DeadlineReader {
    stream: TcpStream,
    /// When the request started reading, and how long it may take.
    deadline: Option<(Stopwatch, Duration)>,
}

impl Read for DeadlineReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if let Some((started, timeout)) = self.deadline {
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                return Err(io::ErrorKind::TimedOut.into());
            }
            self.stream.set_read_timeout(Some(left))?;
        }
        self.stream.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use sapphire_assets::pack::{Codec, PackAsset, write_pack};
    use sapphire_assets::{AssetId, AssetKind};
    use sapphire_time::Stopwatch;

    use super::*;
    use crate::protocol::ByteRange;

    /// Short enough for tests, long enough that a loaded machine still sends a few bytes within.
    const SHORT: Duration = Duration::from_millis(300);

    /// A device of the test's own, listening on loopback: the connection it takes exchanges
    /// preambles, then `serve` plays the device's side of it.
    fn device(serve: impl FnOnce(TcpStream) + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(&Preamble::CURRENT.to_bytes()).unwrap();
            Preamble::read(&mut stream).unwrap().check().unwrap();
            serve(stream);
        });
        address
    }

    /// A status reply of `id`, framed.
    fn status_reply(id: u32) -> Vec<u8> {
        let status =
            DeviceStatus { app: "fake".into(), platform: "loopback".into(), ..DeviceStatus::default() };
        let mut wire = Vec::new();
        write_frame(&mut wire, &Reply { id, body: ReplyBody::Status(status) }).unwrap();
        wire
    }

    #[test]
    fn a_reply_cut_short_by_a_timeout_closes_the_connection() {
        // The device stalls partway through a reply, at every byte of its length and message:
        // the request times out, and the connection is closed rather than read from wherever the
        // stall left it.
        let whole = status_reply(0);
        std::thread::scope(|scope| {
            for split in 1..whole.len() {
                let whole = whole.clone();
                scope.spawn(move || {
                    let address = device(move |mut stream| {
                        // Before the request, so the client's read meets it whatever the timing.
                        stream.write_all(&whole[..split]).unwrap();
                        let mut requests = 0;
                        while read_frame::<Request>(&mut stream).is_ok() {
                            requests += 1;
                            if requests == 2 {
                                // A client reading on would take the rest for a frame of its own.
                                stream.write_all(&whole[split..]).unwrap();
                                stream.write_all(&status_reply(1)).unwrap();
                            }
                        }
                    });
                    let mut client = Client::connect(address).unwrap();
                    let first = client.request(RequestBody::Status, SHORT);
                    assert!(matches!(first, Err(LinkError::TimedOut(_))), "split at {split}: {first:?}");
                    let second = client.request(RequestBody::Status, SHORT);
                    assert!(matches!(second, Err(LinkError::Closed)), "split at {split}: {second:?}");
                });
            }
        });
    }

    #[test]
    fn a_reply_dripping_in_times_out_on_time() {
        // Each byte comes within a read's timeout; the reply as a whole comes long after.
        let whole = status_reply(0);
        let address = device(move |mut stream| {
            let _ = read_frame::<Request>(&mut stream);
            for byte in &whole {
                std::thread::sleep(SHORT * 3 / 5);
                if stream.write_all(std::slice::from_ref(byte)).is_err() {
                    return;
                }
            }
        });
        let mut client = Client::connect(address).unwrap();
        let stopwatch = Stopwatch::start();
        let result = client.request(RequestBody::Status, SHORT);
        let took = stopwatch.elapsed();
        assert!(matches!(result, Err(LinkError::TimedOut(_))), "{result:?} after {took:?}");
        assert!(took < SHORT * 3, "a request ends at its deadline: {took:?}");
        assert!(matches!(client.request(RequestBody::Status, SHORT), Err(LinkError::Closed)));
    }

    #[test]
    fn replies_to_other_requests_dont_stretch_a_request() {
        let address = device(|mut stream| {
            let _ = read_frame::<Request>(&mut stream);
            let stale = status_reply(999);
            let stopwatch = Stopwatch::start();
            // Until the client gives up, or long after it should have.
            while stopwatch.elapsed() < Duration::from_secs(5) && stream.write_all(&stale).is_ok() {
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let mut client = Client::connect(address).unwrap();
        let stopwatch = Stopwatch::start();
        let result = client.request(RequestBody::Status, SHORT);
        let took = stopwatch.elapsed();
        assert!(matches!(result, Err(LinkError::TimedOut(_))), "{result:?} after {took:?}");
        assert!(took < SHORT * 3, "a request ends at its deadline: {took:?}");
        assert!(matches!(client.request(RequestBody::Status, SHORT), Err(LinkError::Closed)));
    }

    /// A pack of `data` bytes and a little more: a valid head, then zeros, which the client
    /// doesn't look into.
    fn large_pack(data: usize) -> Vec<u8> {
        let id = AssetId::from_bytes([1; 16]);
        let asset = PackAsset::encode(id, AssetKind::Rig, Codec::Lz4, &[7; 64], Vec::new(), 3).unwrap();
        let mut pack = write_pack(vec![asset]).unwrap();
        pack.resize(pack.len() + data, 0);
        pack
    }

    /// The device's side of a pack push's start: asks for every byte past the head. Returns the
    /// pack's length.
    fn ask_for_everything(stream: &mut TcpStream) -> u64 {
        let Ok(Request { id, body: RequestBody::PackBegin { len, head, .. } }) = read_frame(stream) else {
            panic!("a pack push begins");
        };
        let data = PackHeader::read(&head).unwrap().data_offset;
        let ranges = vec![ByteRange { offset: data, len: len - data }];
        write_frame(stream, &Reply { id, body: ReplyBody::PackNeed { ranges } }).unwrap();
        len
    }

    #[test]
    fn a_device_that_stops_reading_fails_a_push_once_no_bytes_move() {
        // Far more than loopback's socket buffers hold on any desktop OS (8 MiB on macOS), or the
        // push would be left waiting for its reply instead.
        let pack = large_pack(64 << 20);
        let address = device(|mut stream| {
            ask_for_everything(&mut stream);
            // Holding the connection open, reading nothing, long after the client should give up.
            std::thread::sleep(Duration::from_secs(30));
        });
        let (done, finished) = mpsc::channel();
        std::thread::spawn(move || {
            let mut client = Client::connect(address).unwrap();
            client.set_progress(SHORT);
            let stopwatch = Stopwatch::start();
            let result = client.push_pack("zone", &pack).map(drop);
            let _ = done.send((result, stopwatch.elapsed(), client.status()));
        });
        let (result, took, after) =
            finished.recv_timeout(Duration::from_secs(15)).expect("the push fails rather than blocking");
        assert!(matches!(result, Err(LinkError::Stalled(_))), "{result:?} after {took:?}");
        // The OS counts a send's timeout from the last bytes it took within a write, or from the
        // write's start: one to two deadlines without progress.
        assert!(took < SHORT * 4, "{took:?}");
        assert!(matches!(after, Err(LinkError::Closed)), "{after:?}");
    }

    #[test]
    fn a_device_reading_slowly_takes_a_push_as_long_as_it_keeps_reading() {
        // The device reads a chunk at a time, slowly: the push takes several progress deadlines
        // in all, and never one without progress.
        let pack = large_pack(32 << 20);
        let sent = pack.len() as u64 - PackHeader::read(&pack).unwrap().data_offset;
        let address = device(|mut stream| {
            let len = ask_for_everything(&mut stream);
            let mut received = 0;
            loop {
                match read_frame::<Request>(&mut stream).unwrap() {
                    Request { body: RequestBody::PackChunk { bytes, .. }, .. } => {
                        received += bytes.len() as u64
                    }
                    Request { id, body: RequestBody::PackEnd } => {
                        let outcome = PackOutcome { received, len, ..PackOutcome::default() };
                        write_frame(&mut stream, &Reply { id, body: ReplyBody::PackApplied(outcome) })
                            .unwrap();
                        return;
                    }
                    other => panic!("{other:?}"),
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        });
        let mut client = Client::connect(address).unwrap();
        client.set_progress(SHORT);
        let stopwatch = Stopwatch::start();
        let outcome = client.push_pack("zone", &pack).unwrap();
        let took = stopwatch.elapsed();
        assert_eq!(outcome.received, sent);
        assert!(took > SHORT * 2, "the push outlasted its progress deadline: {took:?}");
    }
}
