//! Saves and settings: postcard after a [`header`].
//!
//! Data from an older schema version upgrades on load ([`Versioned`]). Unlike RON, postcard
//! is positional and records no field names, so serde attributes can't absorb changes here:
//! adding, removing or reordering a field always needs a new version, and
//! `#[serde(skip_serializing_if)]` must never be used on saved types.

use sapphire_reflect::{Decoder, TypePath, VersionError, Versioned, decode_versioned};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::header::{self, HEADER_LEN, HeaderError};

/// Marks save data.
pub const MAGIC: [u8; 4] = *b"SPSV";

/// Save data that can't be read or written.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error("schema version {found} is newer than this build's {current}")]
    TooNew { found: u32, current: u32 },
    #[error("schema version {found} is older than the oldest supported, {oldest}")]
    TooOld { found: u32, oldest: u32 },
    #[error("postcard: {0}")]
    Postcard(#[from] postcard::Error),
    #[error("{0} bytes left over after the data")]
    TrailingBytes(usize),
}

impl From<VersionError<SaveError>> for SaveError {
    fn from(error: VersionError<SaveError>) -> Self {
        match error {
            VersionError::TooNew { found, current } => Self::TooNew { found, current },
            VersionError::TooOld { found, oldest } => Self::TooOld { found, oldest },
            VersionError::Decode(error) => error,
        }
    }
}

/// Writes `value` as save data.
pub fn to_save<T: Versioned + Serialize + TypePath>(value: &T) -> Result<Vec<u8>, SaveError> {
    let mut bytes = postcard::to_extend(value, header::write::<T>(MAGIC, 0).to_vec())?;
    let payload_len = bytes.len() - HEADER_LEN;
    header::set_payload_len(&mut bytes, payload_len);
    Ok(bytes)
}

/// Reads save data holding a `T`, upgrading data from older schema versions.
pub fn from_save<T: Versioned + DeserializeOwned + TypePath>(bytes: &[u8]) -> Result<T, SaveError> {
    let (version, payload) = header::read::<T>(MAGIC, bytes)?;
    Ok(decode_versioned::<T, _>(version, Payload(payload))?)
}

struct Payload<'a>(&'a [u8]);

impl Decoder for Payload<'_> {
    type Error = SaveError;

    fn decode<T: DeserializeOwned>(self) -> Result<T, SaveError> {
        match postcard::take_from_bytes(self.0)? {
            (value, []) => Ok(value),
            (_, rest) => Err(SaveError::TrailingBytes(rest.len())),
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy_reflect::TypePath;
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, TypePath, Serialize, Deserialize)]
    #[type_path = "sapphire::test"]
    struct Progress {
        chapter: u16,
        characters: Vec<String>,
        playtime_seconds: u64,
    }

    impl Versioned for Progress {
        const VERSION: u32 = 2;

        fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
            decode_versioned::<v1::Progress, D>(version, data).map(|old| Progress {
                chapter: old.chapter,
                characters: old.characters,
                playtime_seconds: 0,
            })
        }
    }

    mod v1 {
        use bevy_reflect::TypePath;

        #[derive(TypePath, serde::Serialize, serde::Deserialize)]
        #[type_path = "sapphire::test"]
        #[type_name = "Progress"]
        pub struct Progress {
            pub chapter: u16,
            pub characters: Vec<String>,
        }

        impl sapphire_reflect::Versioned for Progress {
            const VERSION: u32 = 1;
        }
    }

    fn progress() -> Progress {
        Progress { chapter: 3, characters: vec!["Yuzuha".into(), "Kael".into()], playtime_seconds: 7200 }
    }

    #[test]
    fn saves_round_trip() {
        let bytes = to_save(&progress()).expect("write");
        assert_eq!(&bytes[..4], b"SPSV");
        assert_eq!(from_save::<Progress>(&bytes).expect("read"), progress());
    }

    #[test]
    fn old_saves_upgrade() {
        // What a build at version 1 wrote: the same stable name, one field fewer.
        let old = v1::Progress { chapter: 3, characters: vec!["Kael".into()] };
        let bytes = to_save(&old).expect("write");
        let upgraded = from_save::<Progress>(&bytes).expect("upgrade");
        assert_eq!((upgraded.chapter, upgraded.playtime_seconds), (3, 0));
    }

    #[test]
    fn damaged_saves_are_errors() {
        let bytes = to_save(&progress()).expect("write");
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(matches!(
            from_save::<Progress>(&longer),
            Err(SaveError::Header(HeaderError::WrongLength { .. }))
        ));
        // A header that covers the extra byte: postcard reads the value and leaves it over.
        header::set_payload_len(&mut longer, bytes.len() + 1 - HEADER_LEN);
        assert!(matches!(from_save::<Progress>(&longer), Err(SaveError::TrailingBytes(1))));
        let truncated = &bytes[..bytes.len() - 3];
        assert!(from_save::<Progress>(truncated).is_err());
    }
}
