//! One slot's file: a frame around the bytes `sapphire_serialize` makes.
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic, `SPSL` |
//! | 4 | 4 | this layout's format, currently 1 |
//! | 8 | 8 | generation: which slot is newer, and nothing else reads the clock for it |
//! | 16 | 4 | schema version of what the body holds |
//! | 20 | 4 | reserved, zero |
//! | 24 | 8 | body length in bytes |
//! | 32 | 8 | checksum: xxh3-64 of bytes 0..32 and the body |
//! | 40 | | body: `sapphire_serialize::to_save`'s header and `postcard` |
//!
//! Every field is little-endian. A later format keeps all of these in place, the checksum's
//! coverage included, so this build can tell a whole file from a newer build, which it must never
//! overwrite, from a damaged one, which it may. A damaged format field fails the checksum, so
//! flipping a byte can't make a file look newer. The checksum catches damage, not cheating.
//!
//! The body's own header repeats the schema version beside the type's name. The frame's copy lets
//! a write refuse to replace a newer save with no type to decode it as.

use std::fmt;
use std::fs;
use std::io;
use std::path::Path;

use sapphire_serialize::SaveError;
use xxhash_rust::xxh3::Xxh3;

const MAGIC: [u8; 4] = *b"SPSL";
pub(crate) const FORMAT: u32 = 1;
const HEADER_LEN: usize = 40;
/// How much of the header the checksum covers: everything before it.
const CHECKSUMMED_HEADER: usize = 32;

/// One of a store's two files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    pub(crate) const BOTH: [Self; 2] = [Self::A, Self::B];

    /// The file's letter.
    pub(crate) fn letter(self) -> char {
        match self {
            Self::A => 'a',
            Self::B => 'b',
        }
    }

    pub(crate) fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.letter())
    }
}

/// Why a slot's file can't be used.
#[derive(Debug, thiserror::Error)]
pub enum SlotFault {
    #[error("the file couldn't be read: {0}")]
    Unreadable(io::Error),
    #[error("{len} bytes can't hold a {HEADER_LEN}-byte header")]
    Truncated { len: usize },
    #[error("this isn't a save slot: its magic is {0:?}")]
    WrongMagic([u8; 4]),
    #[error("the header says {expected} bytes of body, and {actual} follow it")]
    WrongLength { expected: u64, actual: usize },
    #[error("the checksum doesn't match")]
    BadChecksum,
    /// The file is as it was written, and no migration reads it: a bug in a build, or a version too
    /// old for this one's chain.
    #[error("the data won't decode: {0}")]
    Undecodable(SaveError),
}

/// A slot's file with a good checksum.
#[derive(Debug)]
pub(crate) struct Frame {
    pub(crate) format: u32,
    pub(crate) generation: u64,
    pub(crate) schema: u32,
    pub(crate) body: Vec<u8>,
}

/// What's in a slot's place on disk.
#[derive(Debug)]
pub(crate) enum SlotRead {
    Missing,
    Damaged(SlotFault),
    Whole(Frame),
}

impl SlotRead {
    /// The generation of a whole slot.
    pub(crate) fn generation(&self) -> Option<u64> {
        match self {
            Self::Whole(frame) => Some(frame.generation),
            Self::Missing | Self::Damaged(_) => None,
        }
    }
}

/// Reads and checks a slot's file.
pub(crate) fn read(path: &Path) -> SlotRead {
    match fs::read(path) {
        Ok(bytes) => match parse(bytes) {
            Ok(frame) => SlotRead::Whole(frame),
            Err(fault) => SlotRead::Damaged(fault),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => SlotRead::Missing,
        Err(error) => SlotRead::Damaged(SlotFault::Unreadable(error)),
    }
}

/// A slot's whole file.
pub(crate) fn encode(generation: u64, schema: u32, body: &[u8]) -> Vec<u8> {
    let mut file = Vec::with_capacity(HEADER_LEN + body.len());
    file.extend_from_slice(&MAGIC);
    file.extend_from_slice(&FORMAT.to_le_bytes());
    file.extend_from_slice(&generation.to_le_bytes());
    file.extend_from_slice(&schema.to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&(body.len() as u64).to_le_bytes());
    file.extend_from_slice(&checksum(&file, body).to_le_bytes());
    file.extend_from_slice(body);
    file
}

fn checksum(header: &[u8], body: &[u8]) -> u64 {
    let mut hasher = Xxh3::new();
    hasher.update(&header[..CHECKSUMMED_HEADER]);
    hasher.update(body);
    hasher.digest()
}

fn parse(mut file: Vec<u8>) -> Result<Frame, SlotFault> {
    let Some((header, body)) = file.split_first_chunk::<HEADER_LEN>() else {
        return Err(SlotFault::Truncated { len: file.len() });
    };
    let magic = field::<4>(header, 0);
    if magic != MAGIC {
        return Err(SlotFault::WrongMagic(magic));
    }
    let body_len = u64::from_le_bytes(field(header, 24));
    if body_len != body.len() as u64 {
        return Err(SlotFault::WrongLength { expected: body_len, actual: body.len() });
    }
    if u64::from_le_bytes(field(header, 32)) != checksum(header, body) {
        return Err(SlotFault::BadChecksum);
    }
    let frame = Frame {
        format: u32::from_le_bytes(field(header, 4)),
        generation: u64::from_le_bytes(field(header, 8)),
        schema: u32::from_le_bytes(field(header, 16)),
        body: Vec::new(),
    };
    file.drain(..HEADER_LEN);
    Ok(Frame { body: file, ..frame })
}

fn field<const N: usize>(header: &[u8; HEADER_LEN], offset: usize) -> [u8; N] {
    *header[offset..].first_chunk().expect("fields lie inside the header")
}

/// A whole file as a build with another format would write it.
#[cfg(test)]
pub(crate) fn encode_as_format(format: u32, generation: u64, schema: u32, body: &[u8]) -> Vec<u8> {
    let mut file = encode(generation, schema, body);
    file[4..8].copy_from_slice(&format.to_le_bytes());
    let sum = checksum(&file, body);
    file[CHECKSUMMED_HEADER..HEADER_LEN].copy_from_slice(&sum.to_le_bytes());
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_in_the_documented_layout() {
        let file = encode(7, 3, b"body");
        assert_eq!(&file[0..4], b"SPSL");
        assert_eq!(file[4..8], 1u32.to_le_bytes());
        assert_eq!(file[8..16], 7u64.to_le_bytes());
        assert_eq!(file[16..20], 3u32.to_le_bytes());
        assert_eq!(file[20..24], [0; 4]);
        assert_eq!(file[24..32], 4u64.to_le_bytes());
        assert_eq!(&file[HEADER_LEN..], b"body");
        let frame = parse(file).expect("parse");
        assert_eq!((frame.format, frame.generation, frame.schema), (FORMAT, 7, 3));
        assert_eq!(frame.body, b"body");
    }

    #[test]
    fn an_empty_body_is_still_a_whole_frame() {
        assert!(parse(encode(1, 1, b"")).is_ok());
    }

    #[test]
    fn each_fault_names_itself() {
        assert!(matches!(parse(Vec::new()), Err(SlotFault::Truncated { len: 0 })));
        assert!(matches!(parse(vec![7; 39]), Err(SlotFault::Truncated { len: 39 })));
        assert!(matches!(parse(vec![7; 64]), Err(SlotFault::WrongMagic([7, 7, 7, 7]))));
        let mut file = encode(1, 1, b"body");
        file.pop();
        assert!(matches!(parse(file), Err(SlotFault::WrongLength { expected: 4, actual: 3 })));
        let mut file = encode(1, 1, b"body");
        file[HEADER_LEN] ^= 1;
        assert!(matches!(parse(file), Err(SlotFault::BadChecksum)));
    }

    #[test]
    fn a_huge_length_is_only_a_mismatch() {
        let mut file = encode(1, 1, b"body");
        file[24..32].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(matches!(parse(file), Err(SlotFault::WrongLength { expected: u64::MAX, .. })));
    }
}
