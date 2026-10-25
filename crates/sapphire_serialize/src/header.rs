//! The 32-byte header in front of binary data ([saves](crate::save) and
//! [cooked archives](crate::cooked)).
//!
//! | Offset | Size | Field |
//! |---|---|---|
//! | 0 | 4 | magic: which kind of data follows |
//! | 4 | 4 | header format, currently 1 |
//! | 8 | 8 | [`stable_hash`] of the type's stable name |
//! | 16 | 4 | schema version ([`Versioned::VERSION`]) |
//! | 20 | 4 | reserved, zero |
//! | 24 | 8 | payload length in bytes |
//!
//! Every field is little-endian. 32 bytes keep a payload that follows 16-byte aligned, which
//! rkyv archives need.

use sapphire_reflect::{TypePath, Versioned, stable_hash, stable_name};

pub const HEADER_LEN: usize = 32;

/// The header layout this build reads and writes.
const FORMAT: u32 = 1;

/// A binary header that doesn't match what the reader expects.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum HeaderError {
    #[error("{len} bytes can't hold a {HEADER_LEN}-byte header")]
    Truncated { len: usize },
    #[error(
        "expected {} data, found magic {:?}",
        String::from_utf8_lossy(expected),
        String::from_utf8_lossy(found)
    )]
    WrongMagic { expected: [u8; 4], found: [u8; 4] },
    #[error("header format {0} is not supported (this build reads format {FORMAT})")]
    UnsupportedFormat(u32),
    #[error("the data holds type {found:#018x}, not `{expected}`")]
    WrongType { expected: &'static str, found: u64 },
    #[error("the payload is {actual} bytes, but the header says {expected}")]
    WrongLength { expected: u64, actual: usize },
}

/// The header for a `T` payload of `payload_len` bytes.
pub(crate) fn write<T: TypePath + Versioned>(magic: [u8; 4], payload_len: usize) -> [u8; HEADER_LEN] {
    let mut header = [0; HEADER_LEN];
    header[0..4].copy_from_slice(&magic);
    header[4..8].copy_from_slice(&FORMAT.to_le_bytes());
    header[8..16].copy_from_slice(&stable_hash(stable_name::<T>()).to_le_bytes());
    header[16..20].copy_from_slice(&T::VERSION.to_le_bytes());
    set_payload_len(&mut header, payload_len);
    header
}

/// Sets the payload length of a header written before its payload's size was known.
pub(crate) fn set_payload_len(header: &mut [u8], payload_len: usize) {
    header[24..32].copy_from_slice(&(payload_len as u64).to_le_bytes());
}

/// Checks that `bytes` hold a `T` payload under `magic`, and returns its schema version and
/// the payload.
pub(crate) fn read<T: TypePath>(magic: [u8; 4], bytes: &[u8]) -> Result<(u32, &[u8]), HeaderError> {
    let Some((header, payload)) = bytes.split_first_chunk::<HEADER_LEN>() else {
        return Err(HeaderError::Truncated { len: bytes.len() });
    };
    let found_magic = le_bytes::<4>(header, 0);
    if found_magic != magic {
        return Err(HeaderError::WrongMagic { expected: magic, found: found_magic });
    }
    let format = u32::from_le_bytes(le_bytes(header, 4));
    if format != FORMAT {
        return Err(HeaderError::UnsupportedFormat(format));
    }
    let expected = stable_name::<T>();
    let type_hash = u64::from_le_bytes(le_bytes(header, 8));
    if type_hash != stable_hash(expected) {
        return Err(HeaderError::WrongType { expected, found: type_hash });
    }
    let payload_len = u64::from_le_bytes(le_bytes(header, 24));
    if payload_len != payload.len() as u64 {
        return Err(HeaderError::WrongLength { expected: payload_len, actual: payload.len() });
    }
    Ok((u32::from_le_bytes(le_bytes(header, 16)), payload))
}

/// The `N` bytes of a header field at `offset`.
fn le_bytes<const N: usize>(header: &[u8; HEADER_LEN], offset: usize) -> [u8; N] {
    *header[offset..].first_chunk().expect("fields lie inside the header")
}

#[cfg(test)]
mod tests {
    use bevy_reflect::TypePath;

    use super::*;

    #[derive(TypePath)]
    #[type_path = "sapphire::test"]
    struct Thing;

    impl Versioned for Thing {
        const VERSION: u32 = 7;
    }

    #[derive(TypePath)]
    #[type_path = "sapphire::test"]
    struct Other;

    const MAGIC: [u8; 4] = *b"TEST";

    #[test]
    fn headers_round_trip_and_reject_mismatches() {
        let mut bytes = write::<Thing>(MAGIC, 3).to_vec();
        bytes.extend([1, 2, 3]);
        assert_eq!(read::<Thing>(MAGIC, &bytes), Ok((7, &[1, 2, 3][..])));

        assert_eq!(read::<Thing>(MAGIC, &bytes[..20]), Err(HeaderError::Truncated { len: 20 }));
        assert_eq!(
            read::<Thing>(MAGIC, &bytes[..34]),
            Err(HeaderError::WrongLength { expected: 3, actual: 2 })
        );
        assert!(matches!(read::<Thing>(*b"ELSE", &bytes), Err(HeaderError::WrongMagic { .. })));
        assert!(matches!(
            read::<Other>(MAGIC, &bytes),
            Err(HeaderError::WrongType { expected: "sapphire::test::Other", .. })
        ));
        bytes[4] = 2;
        assert_eq!(read::<Thing>(MAGIC, &bytes), Err(HeaderError::UnsupportedFormat(2)));
    }
}
