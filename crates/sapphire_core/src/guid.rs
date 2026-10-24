//! [`Guid`]: 128-bit identity for assets and editor documents (spec §5.3, rule R5).
//!
//! The runtime only stores, compares and prints GUIDs. Generating them is a tools concern
//! (`.meta` sidecars, editor documents), so there is no constructor that makes a new one.

use std::fmt;
use std::str::FromStr;

/// A 128-bit globally unique identifier.
///
/// The text form is the canonical lowercase `8-4-4-4-12` hex layout, most significant digit
/// first, so `Guid::from_u128(x)` prints like `uuid::Uuid::from_u128(x)`. Parsing also accepts
/// uppercase digits.
///
/// ```
/// use sapphire_core::Guid;
///
/// let guid: Guid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8".parse()?;
/// assert_eq!(guid, Guid::from_u128(0x6ba7b810_9dad_11d1_80b4_00c04fd430c8));
/// assert_eq!(guid.to_string(), "6ba7b810-9dad-11d1-80b4-00c04fd430c8");
/// # Ok::<(), sapphire_core::guid::ParseGuidError>(())
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
#[repr(transparent)]
pub struct Guid(u128);

/// Byte offsets of the hyphens in the canonical text form.
const HYPHENS: [usize; 4] = [8, 13, 18, 23];

/// Length of the canonical text form.
const TEXT_LEN: usize = 36;

impl Guid {
    /// The all-zero GUID, used as "no asset".
    pub const NIL: Self = Self(0);

    /// Wraps a raw 128-bit value.
    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    /// The raw 128-bit value.
    #[must_use]
    pub const fn as_u128(self) -> u128 {
        self.0
    }

    /// Whether this is [`Guid::NIL`].
    #[must_use]
    pub const fn is_nil(self) -> bool {
        self.0 == 0
    }

    /// The canonical text form as ASCII bytes, without allocating.
    fn encode(self) -> [u8; TEXT_LEN] {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = [b'-'; TEXT_LEN];
        let mut shift = 128;
        for (index, byte) in text.iter_mut().enumerate() {
            if HYPHENS.contains(&index) {
                continue;
            }
            shift -= 4;
            *byte = DIGITS[((self.0 >> shift) & 0xf) as usize];
        }
        text
    }
}

impl From<u128> for Guid {
    fn from(value: u128) -> Self {
        Self(value)
    }
}

impl From<Guid> for u128 {
    fn from(guid: Guid) -> Self {
        guid.0
    }
}

impl fmt::Display for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.encode();
        f.pad(std::str::from_utf8(&text).expect("hex digits and hyphens are ASCII"))
    }
}

impl fmt::Debug for Guid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Guid({self})")
    }
}

/// Why a string is not a canonical GUID.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseGuidError {
    /// The input is not exactly 36 bytes long.
    #[error("a GUID is 36 characters (8-4-4-4-12 hex digits), got {0} bytes")]
    InvalidLength(usize),
    /// A hyphen is missing at one of the byte offsets 8, 13, 18 or 23.
    #[error("expected '-' at byte {0}")]
    MissingHyphen(usize),
    /// A byte that should be a hex digit is not one.
    #[error("invalid hex digit at byte {0}")]
    InvalidDigit(usize),
}

impl FromStr for Guid {
    type Err = ParseGuidError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = text.as_bytes();
        if bytes.len() != TEXT_LEN {
            return Err(ParseGuidError::InvalidLength(bytes.len()));
        }
        let mut value = 0u128;
        for (index, &byte) in bytes.iter().enumerate() {
            if HYPHENS.contains(&index) {
                if byte != b'-' {
                    return Err(ParseGuidError::MissingHyphen(index));
                }
                continue;
            }
            let digit = char::from(byte).to_digit(16).ok_or(ParseGuidError::InvalidDigit(index))?;
            value = (value << 4) | u128::from(digit);
        }
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    const SAMPLE: u128 = 0x6ba7_b810_9dad_11d1_80b4_00c0_4fd4_30c8;

    #[test]
    fn display_is_canonical() {
        assert_eq!(Guid::from_u128(SAMPLE).to_string(), "6ba7b810-9dad-11d1-80b4-00c04fd430c8");
        assert_eq!(Guid::NIL.to_string(), "00000000-0000-0000-0000-000000000000");
        assert_eq!(Guid::from_u128(u128::MAX).to_string(), "ffffffff-ffff-ffff-ffff-ffffffffffff");
        assert_eq!(format!("{:?}", Guid::NIL), "Guid(00000000-0000-0000-0000-000000000000)");
        assert_eq!(format!("{:>38}", Guid::NIL), "  00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn round_trips() {
        for value in [0, 1, SAMPLE, u128::MAX, 1 << 127, 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210] {
            let guid = Guid::from_u128(value);
            assert_eq!(guid.to_string().parse::<Guid>(), Ok(guid));
            assert_eq!(guid.as_u128(), value);
            assert_eq!(u128::from(guid), value);
            assert_eq!(Guid::from(value), guid);
        }
    }

    #[test]
    fn parses_uppercase() {
        assert_eq!("6BA7B810-9DAD-11D1-80B4-00C04FD430C8".parse(), Ok(Guid::from_u128(SAMPLE)));
    }

    #[test]
    fn rejects_malformed_text() {
        assert_eq!("".parse::<Guid>(), Err(ParseGuidError::InvalidLength(0)));
        assert_eq!(
            "6ba7b8109dad11d180b400c04fd430c8".parse::<Guid>(),
            Err(ParseGuidError::InvalidLength(32))
        );
        assert_eq!(
            "{6ba7b810-9dad-11d1-80b4-00c04fd430c8}".parse::<Guid>(),
            Err(ParseGuidError::InvalidLength(38))
        );
        assert_eq!(
            "6ba7b810-9dad-11d1-80b4_00c04fd430c8".parse::<Guid>(),
            Err(ParseGuidError::MissingHyphen(23))
        );
        assert_eq!(
            "6ba7b810-9dad-11d1-80b4-00c04fd430cg".parse::<Guid>(),
            Err(ParseGuidError::InvalidDigit(35))
        );
        assert_eq!(
            "6ba7b81--9dad-11d1-80b4-00c04fd430c8".parse::<Guid>(),
            Err(ParseGuidError::InvalidDigit(7))
        );
        // Multi-byte UTF-8 must not be mistaken for digits.
        assert_eq!(
            "6ba7b810-9dad-11d1-80b4-00c04fd430é".parse::<Guid>(),
            Err(ParseGuidError::InvalidDigit(34))
        );
    }

    #[test]
    fn ordering_and_hashing_follow_the_value() {
        assert!(Guid::from_u128(1) < Guid::from_u128(2));
        assert!(Guid::NIL.is_nil());
        assert_eq!(Guid::default(), Guid::NIL);
        let set: HashSet<Guid> = [Guid::from_u128(SAMPLE), Guid::from_u128(SAMPLE), Guid::NIL].into();
        assert_eq!(set.len(), 2);
    }
}
