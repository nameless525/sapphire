//! The desktop half's connection: [`Client`], which pushes to a device and reads its state.

use std::io::{self, BufReader, BufWriter, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::Duration;

use sapphire_assets::pack::{PackError, PackHeader};

use crate::protocol::{
    CHUNK_LEN, Capture, DeviceStatus, LinkError, PackOutcome, Preamble, Reply, ReplyBody, Request,
    RequestBody, SourceFile, read_frame, write_frame,
};

/// How long a device has to answer a request, and to connect. Pushed packs get
/// [`Client::PACK_TIMEOUT`].
pub const TIMEOUT: Duration = Duration::from_secs(10);

/// A connection to a device's live link. Requests go one at a time: each waits for its reply.
#[derive(Debug)]
pub struct Client {
    reader: BufReader<TcpStream>,
    writer: BufWriter<TcpStream>,
    next_id: u32,
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
        stream.set_read_timeout(Some(TIMEOUT))?;
        (&stream).write_all(&Preamble::CURRENT.to_bytes())?;
        Preamble::read(&mut &stream)?.check()?;
        Ok(Self { reader: BufReader::new(stream.try_clone()?), writer: BufWriter::new(stream), next_id: 0 })
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
            let asked = usize::try_from(range.offset)
                .ok()
                .zip(usize::try_from(range.end()).ok())
                .and_then(|(start, end)| bytes.get(start..end))
                .ok_or_else(|| LinkError::PastPack(name.to_owned()))?;
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

    /// Sends `body` and waits up to `timeout` for its reply. An error reply is
    /// [`LinkError::Refused`].
    fn request(&mut self, body: RequestBody, timeout: Duration) -> Result<ReplyBody, LinkError> {
        let id = self.send(body)?;
        self.writer.flush()?;
        self.reader.get_ref().set_read_timeout(Some(timeout))?;
        loop {
            let reply: Reply = read_frame(&mut self.reader).map_err(|error| match error {
                LinkError::Io(error)
                    if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) =>
                {
                    LinkError::TimedOut(timeout)
                }
                other => other,
            })?;
            // Replies to requests given up on (timed out) are skipped.
            if reply.id == id {
                return match reply.body {
                    ReplyBody::Error(message) => Err(LinkError::Refused(message)),
                    body => Ok(body),
                };
            }
        }
    }

    fn send(&mut self, body: RequestBody) -> Result<u32, LinkError> {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        write_frame(&mut self.writer, &Request { id, body })?;
        Ok(id)
    }
}

fn unexpected(expected: &'static str, found: &ReplyBody) -> LinkError {
    LinkError::Unexpected { expected, found: found.name() }
}
