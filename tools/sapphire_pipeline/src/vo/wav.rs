//! WAV masters (M6 U11): what a studio delivers a voice line as, and what `sapphire_soundgen` writes its
//! generated lines as. One channel, in 16- or 24-bit integers or 32-bit floats, at any rate from 8 to 192 kHz;
//! read into `f32` samples exactly (an integer sample over its full scale), so a master means the same samples
//! on every machine. Anything else (stereo, 8 or 32-bit integers, a compressed format, a chunk cut short) is
//! an error saying what was found.

use std::ops::RangeInclusive;

/// The rates a master may be at.
pub const RATES: RangeInclusive<u32> = 8_000..=192_000;

/// `WAVE_FORMAT_PCM`.
const PCM: u16 = 1;
/// `WAVE_FORMAT_IEEE_FLOAT`.
const FLOAT: u16 = 3;
/// `WAVE_FORMAT_EXTENSIBLE`: the format is the first two bytes of the sub-format's GUID.
const EXTENSIBLE: u16 = 0xfffe;

/// How a master's samples are stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    Int16,
    Int24,
    Float32,
}

impl SampleFormat {
    /// As a report names it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Int16 => "16-bit",
            Self::Int24 => "24-bit",
            Self::Float32 => "32-bit float",
        }
    }
}

/// A master, read: one channel's samples, from -1 to 1 for the integer formats.
#[derive(Clone, Debug, PartialEq)]
pub struct Master {
    pub sample_rate: u32,
    pub format: SampleFormat,
    pub samples: Vec<f32>,
}

/// A file that isn't a master this reads.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum WavError {
    #[error("isn't a RIFF WAVE file")]
    NotWave,
    #[error("its `{0}` chunk runs past the end of the file")]
    CutShort(String),
    #[error("has no `{0}` chunk")]
    Missing(&'static str),
    #[error("has {0} channels: a voice master is mono")]
    Channels(u16),
    #[error("is at {0} Hz, not from 8 to 192 kHz")]
    Rate(u32),
    #[error("stores format {format:#06x} at {bits} bits: masters are 16 or 24-bit integers or 32-bit floats")]
    Format { format: u16, bits: u16 },
    #[error("has no samples")]
    Empty,
}

/// Reads a mono master.
pub fn read(bytes: &[u8]) -> Result<Master, WavError> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let (mut fmt, mut data) = (None, None);
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        let body = bytes
            .get(at + 8..at + 8 + len)
            .ok_or_else(|| WavError::CutShort(String::from_utf8_lossy(id).into_owned()))?;
        match id {
            b"fmt " => fmt = Some(body),
            b"data" => data = Some(body),
            _ => {}
        }
        // Chunks are padded to an even length.
        at += 8 + len + (len & 1);
    }
    let fmt = fmt.ok_or(WavError::Missing("fmt "))?;
    let data = data.ok_or(WavError::Missing("data"))?;
    if fmt.len() < 16 {
        return Err(WavError::CutShort("fmt ".to_owned()));
    }
    let word = |at: usize| u16::from_le_bytes([fmt[at], fmt[at + 1]]);
    let mut format = word(0);
    let channels = word(2);
    let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    let bits = word(14);
    if format == EXTENSIBLE {
        format = if fmt.len() >= 26 { word(24) } else { return Err(WavError::CutShort("fmt ".to_owned())) };
    }
    if channels != 1 {
        return Err(WavError::Channels(channels));
    }
    if !RATES.contains(&sample_rate) {
        return Err(WavError::Rate(sample_rate));
    }
    let (sample_format, width) = match (format, bits) {
        (PCM, 16) => (SampleFormat::Int16, 2),
        (PCM, 24) => (SampleFormat::Int24, 3),
        (FLOAT, 32) => (SampleFormat::Float32, 4),
        _ => return Err(WavError::Format { format, bits }),
    };
    let samples: Vec<f32> = data
        .chunks_exact(width)
        .map(|sample| match sample_format {
            SampleFormat::Int16 => f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32_768.0,
            SampleFormat::Int24 => {
                (i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8) as f32 / 8_388_608.0
            }
            SampleFormat::Float32 => f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]),
        })
        .collect();
    if samples.is_empty() {
        return Err(WavError::Empty);
    }
    Ok(Master { sample_rate, format: sample_format, samples })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mono WAV of `format` at `bits`, whose `data` is `data`, with `channels` and `rate` as given.
    fn wav(format: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt ".to_vec();
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&format.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(bits / 8)).to_le_bytes());
        bytes.extend_from_slice(&(bits / 8).to_le_bytes());
        bytes.extend_from_slice(&bits.to_le_bytes());
        // A chunk this reads past, odd-sized so its padding counts.
        bytes.extend_from_slice(b"LIST\x03\0\0\0abc\0");
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    #[test]
    fn floats_read_back_exactly() {
        let samples = [0.0_f32, 0.5, -0.7, 1.0e-9, -1.0];
        let data: Vec<u8> = samples.iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let master = read(&wav(FLOAT, 32, 1, 24_000, &data)).unwrap();
        assert_eq!(
            master,
            Master { sample_rate: 24_000, format: SampleFormat::Float32, samples: samples.to_vec() }
        );
    }

    #[test]
    fn integers_read_over_their_full_scale() {
        let master = read(&wav(PCM, 16, 1, 48_000, &[0x00, 0x80, 0xff, 0x7f, 0x00, 0x40])).unwrap();
        assert_eq!(master.format, SampleFormat::Int16);
        assert_eq!(master.samples, [-1.0, 32_767.0 / 32_768.0, 0.5]);
        let master = read(&wav(PCM, 24, 1, 44_100, &[0x00, 0x00, 0x80, 0x00, 0x00, 0xc0])).unwrap();
        assert_eq!((master.format, master.sample_rate), (SampleFormat::Int24, 44_100));
        assert_eq!(master.samples, [-1.0, -0.5]);
    }

    #[test]
    fn what_isnt_a_mono_master_says_why() {
        assert_eq!(read(b"RIFF\0\0\0\0AVI "), Err(WavError::NotWave));
        assert_eq!(read(&wav(PCM, 16, 2, 48_000, &[0; 4])), Err(WavError::Channels(2)));
        assert_eq!(read(&wav(PCM, 16, 1, 4_000, &[0; 4])), Err(WavError::Rate(4_000)));
        assert_eq!(read(&wav(PCM, 8, 1, 48_000, &[0; 4])), Err(WavError::Format { format: PCM, bits: 8 }));
        assert_eq!(read(&wav(PCM, 16, 1, 48_000, &[])), Err(WavError::Empty));
        let mut short = wav(PCM, 16, 1, 48_000, &[0; 4]);
        short.truncate(short.len() - 1);
        assert_eq!(read(&short), Err(WavError::CutShort("data".to_owned())));
        let mut no_data = wav(PCM, 16, 1, 48_000, &[]);
        no_data.truncate(no_data.len() - 8);
        assert_eq!(read(&no_data), Err(WavError::Missing("data")));
    }
}
