//! Asset identity: [`AssetId`], the GUID every asset is known by (spec §5.10, rule R5).

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An asset's identity: a 128-bit GUID.
///
/// The pipeline gives each source asset one when it first imports it and keeps it in the
/// source's `.meta` sidecar, so the identity survives moves and renames. Cooked data refers
/// to other assets by it. IDs are random, laid out as version 4 UUIDs (RFC 9562) so tools
/// that know UUIDs read them as such. Only tools create them; the runtime only parses and
/// compares them, so shipping code never needs entropy.
///
/// # Forms
///
/// - **Text** (documents, logs): 32 lowercase hex digits in 8-4-4-4-12 groups, the canonical
///   UUID spelling, as in `7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d`. Parsing accepts nothing else
///   (no uppercase, braces or `urn:uuid:`), so an ID has one spelling and a text search for
///   it finds every use.
/// - **Binary serde formats** (postcard saves): the 16 bytes, in text order.
/// - **rkyv archives** (cooked data): the archived form is `AssetId` itself, 16 bytes with
///   alignment 1, so cooked data hands out `&AssetId` in place with no conversion and no
///   padding around it.
#[derive(
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    rkyv::Portable,
    rkyv::bytecheck::CheckBytes,
)]
#[rkyv(as = AssetId)]
#[bytecheck(crate = rkyv::bytecheck)]
#[repr(transparent)]
pub struct AssetId([u8; 16]);

/// Byte offsets of the hyphens in the text form.
const HYPHENS: [usize; 4] = [8, 13, 18, 23];

/// Length of the text form.
const TEXT_LEN: usize = 36;

impl AssetId {
    /// The ID with these bytes, in text order.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// The ID's bytes, in text order.
    pub const fn to_bytes(self) -> [u8; 16] {
        self.0
    }

    /// The text form, as ASCII.
    fn to_text(self) -> [u8; TEXT_LEN] {
        const DIGITS: &[u8; 16] = b"0123456789abcdef";
        let mut text = [b'-'; TEXT_LEN];
        let mut digits = self.0.iter().flat_map(|byte| [byte >> 4, byte & 0xf]);
        for (index, slot) in text.iter_mut().enumerate() {
            if !HYPHENS.contains(&index) {
                *slot = DIGITS[usize::from(digits.next().expect("32 digits fill 32 slots"))];
            }
        }
        text
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = self.to_text();
        f.write_str(std::str::from_utf8(&text).expect("hex digits and hyphens are ASCII"))
    }
}

impl fmt::Debug for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AssetId({self})")
    }
}

/// Text that isn't an asset ID in its canonical form.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ParseAssetIdError {
    #[error("an asset ID is {TEXT_LEN} characters long, not {0}")]
    Length(usize),
    #[error("expected {expected} at character {index} of an asset ID, found {found:?}")]
    Character { index: usize, found: char, expected: &'static str },
}

impl FromStr for AssetId {
    type Err = ParseAssetIdError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let length = text.chars().count();
        if length != TEXT_LEN {
            return Err(ParseAssetIdError::Length(length));
        }
        let mut bytes = [0u8; 16];
        let mut digit = 0;
        for (index, found) in text.chars().enumerate() {
            if HYPHENS.contains(&index) {
                if found != '-' {
                    return Err(ParseAssetIdError::Character { index, found, expected: "`-`" });
                }
                continue;
            }
            let value = match found {
                '0'..='9' => found as u8 - b'0',
                'a'..='f' => found as u8 - b'a' + 10,
                _ => {
                    let expected = "a lowercase hex digit";
                    return Err(ParseAssetIdError::Character { index, found, expected });
                }
            };
            bytes[digit / 2] |= if digit % 2 == 0 { value << 4 } else { value };
            digit += 1;
        }
        Ok(Self(bytes))
    }
}

impl Serialize for AssetId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(std::str::from_utf8(&self.to_text()).expect("ASCII"))
        } else {
            self.0.serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Text;

        impl Visitor<'_> for Text {
            type Value = AssetId;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("an asset ID such as \"7c4b0a9e-3f1d-4c2a-9b8e-5d6f7a8b9c0d\"")
            }

            fn visit_str<E: de::Error>(self, text: &str) -> Result<AssetId, E> {
                text.parse().map_err(E::custom)
            }
        }

        if deserializer.is_human_readable() {
            deserializer.deserialize_str(Text)
        } else {
            <[u8; 16]>::deserialize(deserializer).map(Self)
        }
    }
}

#[cfg(test)]
mod tests {
    use rkyv::rancor;

    use super::*;

    const TEXT: &str = "00112233-4455-6677-8899-aabbccddeeff";

    fn id() -> AssetId {
        AssetId::from_bytes([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
        ])
    }

    #[test]
    fn text_round_trips_in_byte_order() {
        assert_eq!(id().to_string(), TEXT);
        assert_eq!(TEXT.parse(), Ok(id()));
        assert_eq!(format!("{:?}", id()), format!("AssetId({TEXT})"));
        for bytes in [[0; 16], [0xff; 16], *b"sapphire assets!"] {
            let id = AssetId::from_bytes(bytes);
            assert_eq!(id.to_string().parse(), Ok(id));
        }
        // Ordering follows the bytes, and so the text.
        assert!(AssetId::from_bytes([0; 16]) < id() && id() < AssetId::from_bytes([0xff; 16]));
    }

    #[test]
    fn only_the_canonical_spelling_parses() {
        use ParseAssetIdError::{Character, Length};
        let parse = |text: &str| text.parse::<AssetId>();
        assert_eq!(parse(""), Err(Length(0)));
        assert_eq!(parse(&TEXT[1..]), Err(Length(35)));
        assert_eq!(parse(&format!("{TEXT}0")), Err(Length(37)));
        assert_eq!(parse("00112233445566778899aabbccddeeff"), Err(Length(32)), "no hyphens");
        assert_eq!(
            parse(&format!("{{{}}}", &TEXT[1..35])),
            Err(Character { index: 0, found: '{', expected: "a lowercase hex digit" })
        );
        assert_eq!(parse(&format!("urn:uuid:{TEXT}")), Err(Length(45)));

        let upper = TEXT.to_uppercase();
        assert_eq!(
            parse(&upper),
            Err(Character { index: 24, found: 'A', expected: "a lowercase hex digit" })
        );
        let shifted = "0011223-34455-6677-8899-aabbccddeeff";
        assert_eq!(
            parse(shifted),
            Err(Character { index: 7, found: '-', expected: "a lowercase hex digit" })
        );
        let missing = "00112233_4455-6677-8899-aabbccddeeff";
        assert_eq!(parse(missing), Err(Character { index: 8, found: '_', expected: "`-`" }));
        let not_hex = "00112233-4455-6677-8899-aabbccddeefg";
        assert_eq!(
            parse(not_hex),
            Err(Character { index: 35, found: 'g', expected: "a lowercase hex digit" })
        );
        // 36 characters, but not 36 bytes.
        let wide = "00112233-4455-6677-8899-aabbccddeeé";
        assert_eq!(parse(wide), Err(Length(35)));
        let wide = "00112233-4455-6677-8899-aabbccddeeéf";
        assert_eq!(parse(wide), Err(Character { index: 34, found: 'é', expected: "a lowercase hex digit" }));
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Reference {
        target: AssetId,
    }

    #[test]
    fn serde_uses_text_in_documents_and_bytes_in_binary_formats() {
        let reference = Reference { target: id() };
        let text = ron::to_string(&reference).expect("write RON");
        assert_eq!(text, format!("(target:\"{TEXT}\")"));
        assert_eq!(ron::from_str::<Reference>(&text).expect("read RON"), reference);
        let error = ron::from_str::<Reference>("(target: \"not an id\")").expect_err("malformed");
        assert!(error.to_string().contains("36 characters"), "{error}");

        let bytes = postcard::to_allocvec(&reference).expect("write postcard");
        assert_eq!(bytes, id().to_bytes(), "16 bytes, no length prefix");
        assert_eq!(postcard::from_bytes::<Reference>(&bytes).expect("read postcard"), reference);
    }

    #[derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
    struct Cooked {
        flag: u8,
        target: AssetId,
        dependencies: Vec<AssetId>,
    }

    #[test]
    fn archives_hold_asset_ids_in_place() {
        assert_eq!((size_of::<AssetId>(), align_of::<AssetId>()), (16, 1));
        let other = AssetId::from_bytes([7; 16]);
        let cooked = Cooked { flag: 1, target: id(), dependencies: vec![other, id()] };
        let bytes = rkyv::to_bytes::<rancor::Error>(&cooked).expect("archive");
        let archived = rkyv::access::<ArchivedCooked, rancor::Error>(&bytes).expect("validate");
        // The archived field is an `AssetId`, compared and copied directly.
        let target: &AssetId = &archived.target;
        assert_eq!((*target, archived.dependencies.as_slice()), (id(), &[other, id()][..]));
        let back: Cooked = rkyv::deserialize::<Cooked, rancor::Error>(archived).expect("deserialize");
        assert_eq!((back.flag, back.target, back.dependencies), (1, id(), vec![other, id()]));
    }
}
