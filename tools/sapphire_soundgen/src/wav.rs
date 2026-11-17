//! A generated line's master (M6 U11): a WAV file of 32-bit floats, one channel, its samples exactly as they
//! were made, which `sapphire_pipeline vo` reads and encodes as it would a studio's master.

/// `WAVE_FORMAT_IEEE_FLOAT`.
const FLOAT: u16 = 3;

/// The bytes before the samples: `RIFF` and `WAVE`, `fmt ` and its 18 bytes, `fact` and its 4, and `data`'s
/// own header.
pub const HEADER_BYTES: usize = 12 + 26 + 12 + 8;

/// `samples`, one channel at `sample_rate`, as a WAV file of 32-bit floats.
pub fn write_float(sample_rate: u32, samples: &[f32]) -> Vec<u8> {
    let data = u32::try_from(samples.len() * 4).expect("a master under 4 GiB");
    let mut bytes = Vec::with_capacity(HEADER_BYTES + samples.len() * 4);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(HEADER_BYTES as u32 - 8 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&18_u32.to_le_bytes());
    bytes.extend_from_slice(&FLOAT.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 4).to_le_bytes());
    bytes.extend_from_slice(&4_u16.to_le_bytes());
    bytes.extend_from_slice(&32_u16.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    // A format other than PCM states its frames (the WAVE spec's `fact` chunk).
    bytes.extend_from_slice(b"fact");
    bytes.extend_from_slice(&4_u32.to_le_bytes());
    bytes.extend_from_slice(&(data / 4).to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    bytes
}

/// The samples of a master [`write_float`] wrote, and its rate.
#[cfg(test)]
pub fn read_float(bytes: &[u8]) -> (u32, Vec<f32>) {
    assert!(bytes.len() >= HEADER_BYTES && &bytes[..4] == b"RIFF" && &bytes[50..54] == b"data");
    let rate = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]);
    let samples =
        bytes[HEADER_BYTES..].as_chunks::<4>().0.iter().map(|sample| f32::from_le_bytes(*sample)).collect();
    (rate, samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_are_written_exactly_after_a_float_header() {
        let samples = [0.0, 0.5, -0.7, 1.0e-9, -1.0];
        let bytes = write_float(24_000, &samples);
        assert_eq!(bytes.len(), HEADER_BYTES + 20);
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize, bytes.len() - 8);
        assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), FLOAT);
        assert_eq!(read_float(&bytes), (24_000, samples.to_vec()));
    }
}
