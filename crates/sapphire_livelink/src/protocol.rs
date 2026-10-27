//! The wire protocol: a versioned preamble, then length-prefixed postcard frames.
//!
//! # Connecting
//!
//! Whichever side dialed (the tool, or the device behind `adb reverse`), both first write an
//! 8-byte [`Preamble`]: the magic `SPLK` and [`PROTOCOL_VERSION`]. It is fixed bytes, not postcard,
//! so builds of any two versions understand each other's: a device whose version differs from
//! the tool's answers with its own preamble and closes, and the tool says which versions met
//! ([`LinkError::Version`]). Message layouts change only with the version (postcard is
//! positional).
//!
//! # Frames
//!
//! Every message after the preamble is a frame: its length as a little-endian u32, then the
//! message, postcard-encoded, at most [`MAX_FRAME`] bytes. A longer frame is refused unread, so a
//! peer can't make the other allocate more.
//!
//! # Messages
//!
//! The tool sends [`Request`]s; the device answers each with a [`Reply`] of the same ID, except
//! [`RequestBody::PackChunk`], which streams, so a pack's bytes take no round trips. Replies come
//! as requests finish, which for a pushed pack is once the assets that changed are swapped in.
//!
//! A pack is pushed in three steps, so only what the device doesn't have crosses the link:
//!
//! 1. [`RequestBody::PackBegin`] carries the new pack's header and table of contents.
//! 2. The device compares them with the version it serves and answers [`ReplyBody::PackNeed`]:
//!    the byte ranges of the new pack it can't take from the old one (changed and new assets).
//! 3. The tool streams those ranges in order, in chunks of at most [`CHUNK_LEN`]
//!    ([`RequestBody::PackChunk`]), then [`RequestBody::PackEnd`], whose reply says what the
//!    device swapped in ([`ReplyBody::PackApplied`]).
//!
//! A capture ([`RequestBody::Capture`]) is answered once the device has drawn its next frame,
//! with that frame as a PNG ([`Capture`]), so the desktop sees what any device shows without
//! tools of its own (a phone's screenshot tools, a window to look at).

use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

use sapphire_assets::{AssetId, AssetKind};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The protocol this build speaks. Raise it with any change to the preamble, the framing or a
/// message's layout. Version 2 (M1 X2's rehearsal) added [`RequestBody::Capture`]; version 3 (M1
/// A8 follow-up 2) the frames drawn while pushed shaders built
/// ([`ReplyBody::ShadersApplied`]).
pub const PROTOCOL_VERSION: u32 = 3;

/// The port a device listens on and the tool connects to by default.
pub const DEFAULT_PORT: u16 = 7811;

/// The largest frame either side accepts: a chunk, or every shader source at once, with room.
pub const MAX_FRAME: usize = 4 << 20;

/// The largest piece of a pack one [`RequestBody::PackChunk`] carries.
pub const CHUNK_LEN: usize = 256 << 10;

const MAGIC: [u8; 4] = *b"SPLK";

/// What each side writes first: the magic and its protocol version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preamble {
    pub version: u32,
}

impl Preamble {
    pub const LEN: usize = 8;

    /// This build's.
    pub const CURRENT: Self = Self { version: PROTOCOL_VERSION };

    pub fn to_bytes(self) -> [u8; Self::LEN] {
        let mut bytes = [0; Self::LEN];
        bytes[..4].copy_from_slice(&MAGIC);
        bytes[4..].copy_from_slice(&self.version.to_le_bytes());
        bytes
    }

    /// Reads a peer's preamble. Fails if the peer isn't a live link.
    pub fn read(reader: &mut impl Read) -> Result<Self, LinkError> {
        let mut bytes = [0; Self::LEN];
        reader.read_exact(&mut bytes).map_err(LinkError::from_read)?;
        if bytes[..4] != MAGIC {
            return Err(LinkError::NotALiveLink);
        }
        Ok(Self { version: u32::from_le_bytes(*bytes[4..].first_chunk().expect("4 bytes")) })
    }

    /// Fails unless the peer speaks this build's protocol.
    pub fn check(self) -> Result<(), LinkError> {
        if self.version == PROTOCOL_VERSION {
            Ok(())
        } else {
            Err(LinkError::Version { peer: self.version, ours: PROTOCOL_VERSION })
        }
    }
}

/// Why a link failed.
#[derive(Debug, thiserror::Error)]
pub enum LinkError {
    #[error("the connection failed")]
    Io(#[from] io::Error),
    #[error("the connection closed")]
    Closed,
    #[error("the peer isn't a Sapphire live link")]
    NotALiveLink,
    #[error(
        "the peer speaks live link protocol {peer} and this build {ours}: build the app and the tool from \
         the same source"
    )]
    Version { peer: u32, ours: u32 },
    #[error("a frame of {0} bytes is larger than the {MAX_FRAME} allowed")]
    FrameTooLarge(usize),
    #[error("a malformed message")]
    Malformed(#[from] postcard::Error),
    /// The device refused a request, and kept what it had.
    #[error("{0}")]
    Refused(String),
    #[error("the device didn't answer within {} s", .0.as_secs())]
    TimedOut(Duration),
    #[error("the device answered {found} where {expected} was expected")]
    Unexpected { expected: &'static str, found: &'static str },
    #[error("the device asked for bytes past the end of pack {0:?}")]
    PastPack(String),
    #[error("pack {name:?} is invalid: {error}")]
    InvalidPack { name: String, error: sapphire_assets::pack::PackError },
}

impl LinkError {
    /// A read error, with a clean end of stream as [`Self::Closed`].
    fn from_read(error: io::Error) -> Self {
        if error.kind() == io::ErrorKind::UnexpectedEof { Self::Closed } else { Self::Io(error) }
    }
}

/// Writes `message` as one frame.
pub fn write_frame(writer: &mut impl Write, message: &impl Serialize) -> Result<(), LinkError> {
    let bytes = postcard::to_stdvec(message)?;
    if bytes.len() > MAX_FRAME {
        return Err(LinkError::FrameTooLarge(bytes.len()));
    }
    writer.write_all(&(bytes.len() as u32).to_le_bytes())?;
    writer.write_all(&bytes)?;
    Ok(())
}

/// Reads one frame and decodes it as a `T`. A frame longer than [`MAX_FRAME`] is refused before
/// anything is allocated for it.
pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> Result<T, LinkError> {
    let mut len = [0; 4];
    reader.read_exact(&mut len).map_err(LinkError::from_read)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(LinkError::FrameTooLarge(len));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes).map_err(LinkError::from_read)?;
    Ok(postcard::from_bytes(&bytes)?)
}

/// A request from the tool. The device answers it with a [`Reply`] of the same `id`, except a
/// [`RequestBody::PackChunk`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: u32,
    pub body: RequestBody,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RequestBody {
    /// The device's state: [`ReplyBody::Status`].
    Status,
    /// Sets cvar `name` from text, as the dev console does: [`ReplyBody::CvarSet`].
    SetCvar { name: String, value: String },
    /// A RON document, which names its type in its envelope; the app hands it to the owner of
    /// that type, which keeps what it has if the document doesn't parse or validate.
    /// `origin` names it in messages (its file name). [`ReplyBody::DocumentApplied`].
    Document { origin: String, text: String },
    /// Shader sources, by file name: they replace those files' text in what the renderer builds
    /// with, and every program using them is built again, or none is. The device builds them off
    /// its frame thread and answers once the new pipelines are swapped in, so the reply comes
    /// when the edit shows. [`ReplyBody::ShadersApplied`].
    Shaders { files: Vec<SourceFile> },
    /// Starts pushing pack `name`, `len` bytes long: `head` is its header and table of contents
    /// (the bytes before its data). [`ReplyBody::PackNeed`].
    PackBegin { name: String, len: u64, head: Vec<u8> },
    /// The next bytes of the ranges the device asked for, from `offset` in the pack: ranges in
    /// order, each from its start. No reply: a chunk that doesn't fit fails the push, which
    /// [`RequestBody::PackEnd`] reports.
    PackChunk { offset: u64, bytes: Vec<u8> },
    /// Every range is sent: the device assembles the pack and swaps it in.
    /// [`ReplyBody::PackApplied`].
    PackEnd,
    /// The next frame the device draws, as a PNG: [`ReplyBody::Captured`]. Everything applied
    /// before the request came shows in it.
    Capture,
}

impl RequestBody {
    /// The message's name, for errors.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::SetCvar { .. } => "cvar",
            Self::Document { .. } => "document",
            Self::Shaders { .. } => "shaders",
            Self::PackBegin { .. } => "pack",
            Self::PackChunk { .. } => "pack chunk",
            Self::PackEnd => "pack end",
            Self::Capture => "capture",
        }
    }
}

impl fmt::Debug for RequestBody {
    // Chunks and sources are long; their sizes say enough.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status => f.write_str("Status"),
            Self::SetCvar { name, value } => {
                f.debug_struct("SetCvar").field("name", name).field("value", value).finish()
            }
            Self::Document { origin, text } => {
                f.debug_struct("Document").field("origin", origin).field("bytes", &text.len()).finish()
            }
            Self::Shaders { files } => {
                let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
                f.debug_struct("Shaders").field("files", &names).finish()
            }
            Self::PackBegin { name, len, head } => f
                .debug_struct("PackBegin")
                .field("name", name)
                .field("len", len)
                .field("head", &head.len())
                .finish(),
            Self::PackChunk { offset, bytes } => {
                f.debug_struct("PackChunk").field("offset", offset).field("bytes", &bytes.len()).finish()
            }
            Self::PackEnd => f.write_str("PackEnd"),
            Self::Capture => f.write_str("Capture"),
        }
    }
}

/// A source file by name: a WGSL file of the renderer's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceFile {
    pub name: String,
    pub text: String,
}

/// The device's answer to the [`Request`] with the same `id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub id: u32,
    pub body: ReplyBody,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ReplyBody {
    Status(DeviceStatus),
    /// The cvar's value now, as the console prints it.
    CvarSet {
        name: String,
        value: String,
    },
    /// The document replaced its owner's data: `owner` says what it was.
    DocumentApplied {
        owner: String,
    },
    /// The programs and pipelines the new sources built, none if they changed nothing, and the
    /// frames the device drew with the shaders it had while it built them.
    ShadersApplied {
        programs: u32,
        pipelines: u32,
        frames: u32,
    },
    /// The ranges of the pack to send, in order.
    PackNeed {
        ranges: Vec<ByteRange>,
    },
    PackApplied(PackOutcome),
    /// The request failed: the device keeps what it had.
    Error(String),
    /// The frame a [`RequestBody::Capture`] asked for.
    Captured(Capture),
}

impl ReplyBody {
    /// The message's name, for errors.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Status(_) => "status",
            Self::CvarSet { .. } => "cvar set",
            Self::DocumentApplied { .. } => "document applied",
            Self::ShadersApplied { .. } => "shaders applied",
            Self::PackNeed { .. } => "pack need",
            Self::PackApplied(_) => "pack applied",
            Self::Error(_) => "error",
            Self::Captured(_) => "captured",
        }
    }
}

/// The largest PNG a [`Capture`] carries: a frame, less room for the rest of the reply.
pub const MAX_CAPTURE: usize = MAX_FRAME - 1024;

/// A frame the device drew, for [`RequestBody::Capture`]: what the display shows (the overlay
/// included, when it does) as an 8-bit sRGB PNG, at the frame's size, or halved until the PNG
/// fits [`MAX_CAPTURE`].
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    /// The frame's number, as the status counts them.
    pub frame: u64,
    /// The frame's size in pixels.
    pub width: u32,
    pub height: u32,
    /// How many times the image was halved to fit: the PNG is that much smaller than the frame.
    pub halved: u32,
    pub png: Vec<u8>,
}

impl fmt::Debug for Capture {
    // The image is long; its size says enough.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Capture")
            .field("frame", &self.frame)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("halved", &self.halved)
            .field("png", &self.png.len())
            .finish()
    }
}

/// Bytes `offset..offset + len` of a pack.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u64,
}

impl ByteRange {
    pub fn end(&self) -> u64 {
        self.offset + self.len
    }
}

/// What a pushed pack did on the device, once the assets it changed were swapped in.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackOutcome {
    /// The app had the pack mounted, and remounted it; otherwise it only serves the new version
    /// to whatever mounts it later.
    pub mounted: bool,
    /// Bytes the push sent, of the pack's.
    pub received: u64,
    pub len: u64,
    /// The remount's report: assets with other bytes or new, assets no longer in the pack, and
    /// loaded assets that started loading a new version.
    pub changed: u32,
    pub removed: u32,
    pub reloading: u32,
    /// New versions swapped in behind their handles, with the generation each now is.
    pub reloaded: Vec<AssetGeneration>,
    /// New versions that didn't load: those assets keep the versions they had.
    pub failed: Vec<AssetFailure>,
}

/// An asset's [generation](sapphire_assets::Upload::generation): how many versions replaced
/// the first behind its handles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetGeneration {
    pub id: AssetId,
    pub kind: AssetKind,
    pub generation: u32,
}

/// An asset whose new version failed, and why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetFailure {
    pub id: AssetId,
    pub kind: AssetKind,
    pub error: String,
}

/// A snapshot of the device, for [`RequestBody::Status`].
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceStatus {
    /// The app's name and version.
    pub app: String,
    /// The OS it runs on, and whether that is a simulator or emulator.
    pub platform: String,
    /// The content cook it reads: `desktop` or `mobile`, as `cargo xtask content --cook` names
    /// them.
    pub cook: String,
    /// Frames since the app started, and the last one's length in microseconds.
    pub frame: u64,
    pub frame_time_us: u32,
    pub assets: AssetCounts,
    /// Every asset hot reload or live link replaced since the app started, with the generation
    /// it now is.
    pub generations: Vec<AssetGeneration>,
    /// Requests the device applied and refused, and pack bytes received, over every connection.
    pub applied: u32,
    pub refused: u32,
    pub received: u64,
}

/// The asset server's counters (`sapphire_assets::AssetStats`), as the status carries them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetCounts {
    pub assets: u32,
    pub ready: u32,
    pub loading: u32,
    pub failed: u32,
    /// Ready assets loading a new version.
    pub reloading: u32,
    /// New versions swapped in, and those that failed, since the app started.
    pub reloads: u64,
    pub reload_failures: u64,
}

impl From<sapphire_assets::AssetStats> for AssetCounts {
    fn from(stats: sapphire_assets::AssetStats) -> Self {
        Self {
            assets: stats.assets,
            ready: stats.ready,
            loading: stats.loading + stats.waiting,
            failed: stats.failed,
            reloading: stats.reloading,
            reloads: stats.reloads,
            reload_failures: stats.reload_failures,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<T: Serialize + DeserializeOwned>(message: &T) -> T {
        let mut wire = Vec::new();
        write_frame(&mut wire, message).unwrap();
        let len = u32::from_le_bytes(*wire.first_chunk().unwrap()) as usize;
        assert_eq!(len, wire.len() - 4, "a length prefix, then the message");
        let mut reader = wire.as_slice();
        let read = read_frame(&mut reader).unwrap();
        assert!(reader.is_empty(), "exactly one frame read");
        read
    }

    #[test]
    fn frames_carry_every_message() {
        let requests = [
            RequestBody::Status,
            RequestBody::SetCvar { name: "post.filter_strength".into(), value: "0.5".into() },
            RequestBody::Document { origin: "default_day.ron".into(), text: "(type: \"x\")".into() },
            RequestBody::Shaders {
                files: vec![SourceFile { name: "sky.wgsl".into(), text: "// sky".into() }],
            },
            RequestBody::PackBegin { name: "shared".into(), len: 4096, head: vec![1; 300] },
            RequestBody::PackChunk { offset: 300, bytes: vec![7; CHUNK_LEN] },
            RequestBody::PackEnd,
            RequestBody::Capture,
        ];
        for (id, body) in requests.into_iter().enumerate() {
            let request = Request { id: id as u32, body };
            assert_eq!(round_trip(&request), request);
        }
        let outcome = PackOutcome {
            mounted: true,
            received: 1200,
            len: 9000,
            changed: 1,
            reloading: 1,
            reloaded: vec![AssetGeneration {
                id: AssetId::from_bytes([3; 16]),
                kind: AssetKind::Material,
                generation: 2,
            }],
            ..PackOutcome::default()
        };
        let replies = [
            ReplyBody::Status(DeviceStatus {
                app: "Sandbox 0.1.0".into(),
                frame: 9,
                ..DeviceStatus::default()
            }),
            ReplyBody::CvarSet { name: "a.b".into(), value: "1".into() },
            ReplyBody::PackNeed { ranges: vec![ByteRange { offset: 512, len: 64 }] },
            ReplyBody::PackApplied(outcome),
            ReplyBody::Error("no".into()),
            ReplyBody::Captured(Capture { frame: 7, width: 640, height: 360, halved: 0, png: vec![9; 4000] }),
            // The largest a capture carries still fits a frame.
            ReplyBody::Captured(Capture {
                frame: u64::MAX,
                width: u32::MAX,
                height: u32::MAX,
                halved: u32::MAX,
                png: vec![1; MAX_CAPTURE],
            }),
        ];
        for (id, body) in replies.into_iter().enumerate() {
            let reply = Reply { id: id as u32, body };
            assert_eq!(round_trip(&reply), reply);
        }
    }

    #[test]
    fn frames_too_large_are_refused_unread() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&(MAX_FRAME as u32 + 1).to_le_bytes());
        let error = read_frame::<Request>(&mut wire.as_slice()).unwrap_err();
        assert!(matches!(error, LinkError::FrameTooLarge(len) if len == MAX_FRAME + 1), "{error}");
        let huge = RequestBody::PackChunk { offset: 0, bytes: vec![0; MAX_FRAME] };
        let error = write_frame(&mut Vec::new(), &Request { id: 1, body: huge }).unwrap_err();
        assert!(matches!(error, LinkError::FrameTooLarge(_)), "{error}");
    }

    #[test]
    fn truncated_and_malformed_frames_are_errors() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &Request { id: 4, body: RequestBody::Status }).unwrap();
        assert!(matches!(read_frame::<Request>(&mut &wire[..wire.len() - 1]), Err(LinkError::Closed)));
        assert!(matches!(read_frame::<Request>(&mut &wire[..2]), Err(LinkError::Closed)));
        assert!(matches!(read_frame::<Request>(&mut [].as_slice()), Err(LinkError::Closed)));
        // A variant past the last one.
        let garbage = [2, 0, 0, 0, 1, 99];
        assert!(matches!(read_frame::<Request>(&mut garbage.as_slice()), Err(LinkError::Malformed(_))));
    }

    #[test]
    fn preambles_name_the_protocol_and_refuse_other_versions() {
        let bytes = Preamble::CURRENT.to_bytes();
        assert_eq!(&bytes[..4], b"SPLK");
        let read = Preamble::read(&mut bytes.as_slice()).unwrap();
        assert_eq!(read, Preamble::CURRENT);
        read.check().unwrap();

        let newer = Preamble { version: PROTOCOL_VERSION + 1 }.to_bytes();
        let error = Preamble::read(&mut newer.as_slice()).unwrap().check().unwrap_err();
        assert!(
            matches!(error, LinkError::Version { peer, ours } if peer == PROTOCOL_VERSION + 1 && ours == PROTOCOL_VERSION)
        );
        assert!(error.to_string().contains("same source"), "{error}");

        let http = *b"GET / HT";
        assert!(matches!(Preamble::read(&mut http.as_slice()), Err(LinkError::NotALiveLink)));
        assert!(matches!(Preamble::read(&mut b"SP".as_slice()), Err(LinkError::Closed)));
    }
}
