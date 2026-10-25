//! Cooked runtime data: rkyv 0.8 archives after a [`header`], read in place.
//!
//! The cooker writes these and the runtime reads the archive where it lies, in a memory map
//! or a loaded buffer, without deserializing. [`access`] validates the archive with
//! bytecheck first, so corrupt or hostile data is an error, never undefined behavior.
//! [`access_unchecked`] skips that walk for data whose integrity was proven another way: a
//! pack whose blake3 hash matched its signed manifest (spec §5.9).
//!
//! Versions must match exactly. Cooked data is rebuilt from its sources when its type
//! changes, so there are no upgrades; older data is [`CookedError::Stale`].
//!
//! The format is rkyv's default: little-endian, aligned, 32-bit relative pointers, the same
//! on every target. The bytes must start 16-byte aligned, as memory maps and
//! [`AlignedVec`]s do.

use rkyv::api::high::{HighSerializer, HighValidator};
use rkyv::bytecheck::CheckBytes;
use rkyv::rancor;
use rkyv::ser::allocator::ArenaHandle;
pub use rkyv::util::AlignedVec;
use rkyv::{Archive, Portable};
use sapphire_reflect::{TypePath, Versioned};

use crate::header::{self, HEADER_LEN, HeaderError};

/// Marks cooked data.
pub const MAGIC: [u8; 4] = *b"SPCK";

/// The alignment cooked bytes must start at.
pub const ALIGN: usize = 16;

/// Cooked data that can't be read or written.
#[derive(Debug, thiserror::Error)]
pub enum CookedError {
    #[error(transparent)]
    Header(#[from] HeaderError),
    #[error("cooked with schema version {found}, but this build reads {current}; recook it")]
    Stale { found: u32, current: u32 },
    #[error("cooked bytes must start {ALIGN}-byte aligned")]
    Misaligned,
    /// A failed bytecheck validation, or a failure while writing.
    #[error("rkyv: {0}")]
    Rkyv(#[from] rancor::Error),
}

/// Writes `value` as cooked data.
pub fn to_cooked<T>(value: &T) -> Result<AlignedVec, CookedError>
where
    T: Versioned
        + TypePath
        + for<'a> rkyv::Serialize<HighSerializer<AlignedVec, ArenaHandle<'a>, rancor::Error>>,
{
    let mut bytes = AlignedVec::new();
    bytes.extend_from_slice(&header::write::<T>(MAGIC, 0));
    // Archives use relative pointers, and the header keeps the archive's alignment, so it can
    // be written in place after the header.
    let mut bytes = rkyv::api::high::to_bytes_in::<_, rancor::Error>(value, bytes)?;
    let payload_len = bytes.len() - HEADER_LEN;
    header::set_payload_len(&mut bytes, payload_len);
    Ok(bytes)
}

/// Reads cooked `T` data in place, after validating it.
pub fn access<T>(bytes: &[u8]) -> Result<&T::Archived, CookedError>
where
    T: Versioned + TypePath + Archive,
    T::Archived: for<'a> CheckBytes<HighValidator<'a, rancor::Error>>,
{
    Ok(rkyv::access::<T::Archived, rancor::Error>(payload::<T>(bytes)?)?)
}

/// Reads cooked `T` data in place without validating the archive. The header is still
/// checked.
///
/// # Safety
///
/// `bytes` must be what [`to_cooked`] wrote for a `T`, unchanged since: for example, part of
/// a pack whose blake3 hash matched its signed manifest.
pub unsafe fn access_unchecked<T>(bytes: &[u8]) -> Result<&T::Archived, CookedError>
where
    T: Versioned + TypePath + Archive,
    T::Archived: Portable,
{
    let payload = payload::<T>(bytes)?;
    // SAFETY: the caller guarantees `bytes` hold a valid archive of `T`, and `payload` checked
    // the alignment it needs.
    Ok(unsafe { rkyv::access_unchecked::<T::Archived>(payload) })
}

/// The archive inside cooked `T` data, once the header and alignment check out.
fn payload<T: Versioned + TypePath>(bytes: &[u8]) -> Result<&[u8], CookedError> {
    if !(bytes.as_ptr() as usize).is_multiple_of(ALIGN) {
        return Err(CookedError::Misaligned);
    }
    let (version, payload) = header::read::<T>(MAGIC, bytes)?;
    if version != T::VERSION {
        return Err(CookedError::Stale { found: version, current: T::VERSION });
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use bevy_reflect::TypePath;

    use super::*;

    #[derive(TypePath, rkyv::Archive, rkyv::Serialize)]
    #[type_path = "sapphire::test"]
    struct Clip {
        name: String,
        times: Vec<f32>,
        looping: bool,
    }

    impl Versioned for Clip {
        const VERSION: u32 = 4;
    }

    #[derive(TypePath, rkyv::Archive, rkyv::Serialize)]
    #[type_path = "sapphire::test"]
    #[type_name = "Clip"]
    struct OldClip {
        name: String,
    }

    impl Versioned for OldClip {
        const VERSION: u32 = 3;
    }

    fn clip() -> Clip {
        Clip { name: "dodge".into(), times: vec![0.0, 0.1, 0.25], looping: false }
    }

    #[test]
    fn cooked_data_is_read_in_place() {
        let bytes = to_cooked(&clip()).expect("cook");
        assert_eq!(&bytes[..4], b"SPCK");
        let archived = access::<Clip>(&bytes).expect("access");
        assert_eq!((archived.name.as_str(), archived.times.len(), archived.looping), ("dodge", 3, false));
        assert_eq!(archived.times[2], 0.25);
        // SAFETY: `bytes` came from `to_cooked` just above.
        let unchecked = unsafe { access_unchecked::<Clip>(&bytes) }.expect("access");
        assert_eq!(unchecked.name.as_str(), "dodge");
    }

    #[test]
    fn stale_misaligned_and_corrupt_data_are_errors() {
        let old = to_cooked(&OldClip { name: "dodge".into() }).expect("cook");
        assert!(matches!(access::<Clip>(&old), Err(CookedError::Stale { found: 3, current: 4 })));

        let bytes = to_cooked(&clip()).expect("cook");
        let mut shifted = AlignedVec::<16>::new();
        shifted.extend_from_slice(&[0]);
        shifted.extend_from_slice(&bytes);
        assert!(matches!(access::<Clip>(&shifted[1..]), Err(CookedError::Misaligned)));

        // Flip every byte of the archive in turn. Some flips are harmless (a float's bits), but
        // none may panic or read out of bounds.
        let mut rejected = 0;
        for index in HEADER_LEN..bytes.len() {
            let mut corrupt = bytes.clone();
            corrupt[index] ^= 0xff;
            rejected += usize::from(access::<Clip>(&corrupt).is_err());
        }
        assert!(rejected > 0);
        // All ones makes `looping` an invalid bool, whatever the layout.
        let mut corrupt = bytes.clone();
        corrupt[HEADER_LEN..].fill(0xff);
        assert!(matches!(access::<Clip>(&corrupt), Err(CookedError::Rkyv(_))));
    }
}
