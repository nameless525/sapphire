//! Just enough of protobuf's wire format to read a tombstone: fields by number, no schema compiled in.

use anyhow::{bail, ensure};

/// A field's value, by wire type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Value<'a> {
    Varint(u64),
    Fixed64(u64),
    /// A string, bytes, an embedded message or a packed repeated field.
    Bytes(&'a [u8]),
    Fixed32(u32),
}

impl<'a> Value<'a> {
    pub fn varint(self) -> Option<u64> {
        match self {
            Self::Varint(value) => Some(value),
            _ => None,
        }
    }

    pub fn bytes(self) -> Option<&'a [u8]> {
        match self {
            Self::Bytes(bytes) => Some(bytes),
            _ => None,
        }
    }

    pub fn string(self) -> Option<String> {
        self.bytes().map(|bytes| String::from_utf8_lossy(bytes).into_owned())
    }
}

/// Every field of the message `bytes` holds, in order, as `(number, value)`. Malformed input is an error.
pub(super) fn fields(mut bytes: &[u8]) -> anyhow::Result<Vec<(u32, Value<'_>)>> {
    let mut fields = Vec::new();
    while !bytes.is_empty() {
        let key = varint(&mut bytes)?;
        let number = u32::try_from(key >> 3)?;
        ensure!(number != 0, "a field numbered 0");
        let value = match key & 7 {
            0 => Value::Varint(varint(&mut bytes)?),
            1 => Value::Fixed64(u64::from_le_bytes(take(&mut bytes, 8)?.try_into()?)),
            2 => {
                let len = usize::try_from(varint(&mut bytes)?)?;
                Value::Bytes(take(&mut bytes, len)?)
            }
            5 => Value::Fixed32(u32::from_le_bytes(take(&mut bytes, 4)?.try_into()?)),
            other => bail!("field {number} has wire type {other}, which no tombstone uses"),
        };
        fields.push((number, value));
    }
    Ok(fields)
}

fn varint(bytes: &mut &[u8]) -> anyhow::Result<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let Some((&byte, rest)) = bytes.split_first() else { bail!("a varint ends early") };
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    bail!("a varint longer than 10 bytes")
}

fn take<'a>(bytes: &mut &'a [u8], len: usize) -> anyhow::Result<&'a [u8]> {
    ensure!(bytes.len() >= len, "a field runs past its message");
    let (taken, rest) = bytes.split_at(len);
    *bytes = rest;
    Ok(taken)
}

/// Writes messages, for tests to build tombstones with.
#[cfg(test)]
pub(super) mod write {
    pub fn varint(out: &mut Vec<u8>, mut value: u64) {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    pub fn uint(out: &mut Vec<u8>, number: u32, value: u64) {
        varint(out, u64::from(number) << 3);
        varint(out, value);
    }

    pub fn bytes(out: &mut Vec<u8>, number: u32, value: &[u8]) {
        varint(out, (u64::from(number) << 3) | 2);
        varint(out, value.len() as u64);
        out.extend_from_slice(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes_and_refuses_the_rest() {
        let mut message = Vec::new();
        write::uint(&mut message, 1, 300);
        write::bytes(&mut message, 4, b"hello");
        message.extend_from_slice(&[(9 << 3) | 1, 1, 0, 0, 0, 0, 0, 0, 0]);
        let read = fields(&message).unwrap();
        assert_eq!(read, [(1, Value::Varint(300)), (4, Value::Bytes(b"hello")), (9, Value::Fixed64(1))]);
        assert!(fields(&message[..message.len() - 1]).is_err());
        assert!(fields(&[0x0b]).is_err(), "wire type 3, a group");
        assert!(fields(&[0x08, 0x80]).is_err(), "a varint cut short");
    }
}
