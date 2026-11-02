//! Cooked sounds (`.ssound`, M2 E1, spec §5.15): an Ogg Vorbis file as it was authored, with what
//! the cooker learned by decoding all of it.
//!
//! The target format is Vorbis on every platform (spec §5.27's table), so the cook copies the file
//! rather than transcoding it, and packs store its blocks raw: Vorbis doesn't compress further.
//! [`Sound::from_vorbis`] is the cooker's check and [`ArchivedSound::decode`] the runtime's load,
//! through one decoder, [`decode_vorbis`], so what cooks is what plays. `lewton` is pure Rust with
//! no unsafe code, so a bad file is an error rather than undefined behavior.
//!
//! Decoding is load-time work (under 1/300 of a sound's length on an Apple M5 core, by
//! `sapphire_audio`'s `tests/costs.rs`), done before the first play, and never on the audio callback.
//! A mixer holds the decoded [`DecodedSound`], so playing costs no decoding at all.

use std::io::Cursor;

use bevy_reflect::TypePath;
use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

/// Conventional file extension for cooked sounds.
pub const SOUND_EXTENSION: &str = "ssound";

/// The pack the cooker puts sounds and cues in, which an app that plays sound mounts. Every other
/// app never reads it, so what it holds costs them nothing but the disk.
pub const AUDIO_PACK: &str = "audio";

/// The longest sound a cook accepts, in frames: ten minutes at 48 kHz. Sounds are decoded into
/// memory whole, so a longer one needs streaming (music, M3).
pub const MAX_SOUND_FRAMES: u32 = 48_000 * 600;

/// Sample rates a sound may have, in hertz.
pub const SOUND_SAMPLE_RATES: std::ops::RangeInclusive<u32> = 8_000..=192_000;

/// A cooked sound. See the [module docs](self).
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct Sound {
    /// Frames per second.
    pub sample_rate: u32,
    /// 1 (mono) or 2 (stereo).
    pub channels: u32,
    /// Decoded length, in frames.
    pub frames: u32,
    /// The Ogg Vorbis file, byte for byte.
    pub vorbis: Vec<u8>,
}

impl Versioned for Sound {
    const VERSION: u32 = 1;
}

/// A sound as a mixer takes it: interleaved samples, nominally within -1 to 1.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedSound {
    /// Frames per second.
    pub sample_rate: u32,
    /// 1 (mono) or 2 (stereo).
    pub channels: u32,
    /// `channels` samples per frame.
    pub samples: Vec<f32>,
}

impl DecodedSound {
    /// Length in frames.
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

/// A sound that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum SoundError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("the Vorbis stream can't be decoded: {0}")]
    Decode(String),
    #[error("{channels} channels: a sound is mono or stereo")]
    Channels { channels: u32 },
    #[error("sample rate {rate} Hz is outside {min} to {max}", min = SOUND_SAMPLE_RATES.start(), max = SOUND_SAMPLE_RATES.end())]
    SampleRate { rate: u32 },
    #[error("the sound holds no audio")]
    Silent,
    #[error("{frames} frames are more than the {MAX_SOUND_FRAMES} a sound may hold")]
    TooLong { frames: u64 },
    #[error("the stream changes from {expected} channels to {found} partway")]
    ChannelsChange { expected: u32, found: u32 },
    #[error("decoded {found} frames, but the cooked sound says {expected}")]
    Length { expected: u32, found: u64 },
}

impl Sound {
    /// Decodes all of `vorbis` to check it and learn its format: what a cook does with a source.
    pub fn from_vorbis(vorbis: Vec<u8>) -> Result<Self, SoundError> {
        let decoded = decode_vorbis(&vorbis)?;
        Ok(Self {
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            frames: decoded.frames() as u32,
            vorbis,
        })
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and the format's numbers.
    /// The stream itself is checked by decoding it ([`ArchivedSound::decode`]).
    pub fn read(bytes: &[u8]) -> Result<&ArchivedSound, SoundError> {
        let sound = sapphire_serialize::access::<Self>(bytes)?;
        check_format(sound.sample_rate.to_native(), sound.channels.to_native())?;
        if sound.frames == 0 || sound.vorbis.is_empty() {
            return Err(SoundError::Silent);
        }
        let frames = sound.frames.to_native();
        if frames > MAX_SOUND_FRAMES {
            return Err(SoundError::TooLong { frames: u64::from(frames) });
        }
        Ok(sound)
    }
}

impl ArchivedSound {
    /// Decodes the stream, and checks it is the length the cook found. Load-time work: see the
    /// [module docs](self).
    pub fn decode(&self) -> Result<DecodedSound, SoundError> {
        let decoded = decode_vorbis(&self.vorbis)?;
        let expected = self.frames.to_native();
        if decoded.frames() as u64 != u64::from(expected) {
            return Err(SoundError::Length { expected, found: decoded.frames() as u64 });
        }
        Ok(decoded)
    }
}

/// Decodes a whole Ogg Vorbis file.
pub fn decode_vorbis(vorbis: &[u8]) -> Result<DecodedSound, SoundError> {
    let decode_error = |error: lewton::VorbisError| SoundError::Decode(error.to_string());
    let mut reader = OggStreamReader::new(Cursor::new(vorbis)).map_err(decode_error)?;
    let channels = u32::from(reader.ident_hdr.audio_channels);
    let sample_rate = reader.ident_hdr.audio_sample_rate;
    check_format(sample_rate, channels)?;
    let mut samples = Vec::new();
    while let Some(packet) =
        reader.read_dec_packet_generic::<InterleavedSamples<f32>>().map_err(decode_error)?
    {
        if packet.channel_count as u32 != channels {
            return Err(SoundError::ChannelsChange {
                expected: channels,
                found: packet.channel_count as u32,
            });
        }
        samples.extend_from_slice(&packet.samples);
        let frames = samples.len() as u64 / u64::from(channels);
        if frames > u64::from(MAX_SOUND_FRAMES) {
            return Err(SoundError::TooLong { frames });
        }
    }
    if samples.is_empty() {
        return Err(SoundError::Silent);
    }
    Ok(DecodedSound { sample_rate, channels, samples })
}

fn check_format(sample_rate: u32, channels: u32) -> Result<(), SoundError> {
    if !(1..=2).contains(&channels) {
        return Err(SoundError::Channels { channels });
    }
    if !SOUND_SAMPLE_RATES.contains(&sample_rate) {
        return Err(SoundError::SampleRate { rate: sample_rate });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    /// One of the Kenney sounds the repository holds.
    fn sample() -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/kenney_impact_sounds/Audio/impactMetal_heavy_000.ogg");
        fs::read(path).expect("the sample sound")
    }

    #[test]
    fn a_cooked_sound_reads_and_decodes_to_what_the_cook_found() {
        let sound = Sound::from_vorbis(sample()).unwrap();
        assert!(sound.frames > 0, "the sample has audio");
        assert!(sound.channels == 1 || sound.channels == 2);
        let cooked = sound.to_cooked().unwrap();
        let read = Sound::read(&cooked).unwrap();
        assert_eq!(read.vorbis.as_slice(), sound.vorbis.as_slice());
        let decoded = read.decode().unwrap();
        assert_eq!(decoded.frames() as u32, sound.frames);
        assert_eq!((decoded.sample_rate, decoded.channels), (sound.sample_rate, sound.channels));
        assert!(decoded.samples.iter().all(|sample| sample.is_finite()));
        assert!(decoded.samples.iter().any(|sample| sample.abs() > 0.01), "it isn't silence");
    }

    #[test]
    fn what_is_not_vorbis_is_an_error() {
        assert!(matches!(Sound::from_vorbis(b"not an ogg file".to_vec()), Err(SoundError::Decode(_))));
        assert!(matches!(Sound::from_vorbis(Vec::new()), Err(SoundError::Decode(_))));
        // Cut through its headers: never a panic, whatever is left.
        let whole = sample();
        for len in [4, 28, 64, 256, 1024] {
            assert!(Sound::from_vorbis(whole[..len.min(whole.len())].to_vec()).is_err(), "{len} bytes");
        }
    }

    #[test]
    fn a_read_refuses_numbers_the_runtime_could_not_use() {
        let good = Sound::from_vorbis(sample()).unwrap();
        let cooked = |edit: fn(&mut Sound)| {
            let mut sound = good.clone();
            edit(&mut sound);
            sound.to_cooked().unwrap()
        };
        let channels = cooked(|sound| sound.channels = 6);
        assert!(matches!(Sound::read(&channels), Err(SoundError::Channels { channels: 6 })));
        let rate = cooked(|sound| sound.sample_rate = 1);
        assert!(matches!(Sound::read(&rate), Err(SoundError::SampleRate { rate: 1 })));
        let empty = cooked(|sound| sound.frames = 0);
        assert!(matches!(Sound::read(&empty), Err(SoundError::Silent)));
        let long = cooked(|sound| sound.frames = MAX_SOUND_FRAMES + 1);
        assert!(matches!(Sound::read(&long), Err(SoundError::TooLong { .. })));
        // The numbers pass but the stream is shorter than they say.
        let short = cooked(|sound| sound.frames += 1);
        let error = Sound::read(&short).unwrap().decode().unwrap_err();
        assert!(matches!(error, SoundError::Length { .. }));
    }
}
