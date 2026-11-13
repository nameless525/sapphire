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
//!
//! A voice sound also carries its mouth curve (M5 C6, [`Sound::mouth`]): what a face says it with, cooked
//! from the decoded samples by [`MouthCurve::analyze`] when its sidecar asks, and read with the sound.

use std::collections::HashSet;
use std::io::Cursor;

use bevy_reflect::TypePath;
use lewton::inside_ogg::OggStreamReader;
use lewton::samples::InterleavedSamples;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::mouth::{ArchivedMouthCurve, MouthCurve};

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
    /// A voice's mouth through it (M5 C6), [`MouthCurve::frames_for`] its length. None for any other sound.
    pub mouth: Option<MouthCurve>,
}

impl Versioned for Sound {
    /// 1: M2 E1. 2: M5 C6's mouth curve.
    const VERSION: u32 = 2;
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
    #[error("the stream changes from {expected} Hz to {found} Hz partway")]
    SampleRateChange { expected: u32, found: u32 },
    #[error("decoded {found} Hz, but the cooked sound says {expected} Hz")]
    SampleRateMismatch { expected: u32, found: u32 },
    #[error("decoded {found} channels, but the cooked sound says {expected}")]
    ChannelsMismatch { expected: u32, found: u32 },
    #[error("decoded {found} frames, but the cooked sound says {expected}")]
    Length { expected: u32, found: u64 },
    #[error("the mouth curve has {found} frames, and a sound this long has {expected}")]
    MouthLength { expected: usize, found: usize },
}

impl Sound {
    /// Decodes all of `vorbis` to check it and learn its format: what a cook does with a source.
    pub fn from_vorbis(vorbis: Vec<u8>) -> Result<Self, SoundError> {
        let decoded = walk_vorbis(&vorbis, |_| {})?;
        Ok(Self {
            sample_rate: decoded.sample_rate,
            channels: decoded.channels,
            frames: decoded.frames,
            vorbis,
            mouth: None,
        })
    }

    /// The sound with its mouth curve, from decoding it again: what a cook does with a voice's.
    pub fn with_mouth(self) -> Result<Self, SoundError> {
        let mouth = MouthCurve::analyze(&self.decode()?);
        Ok(Self { mouth: Some(mouth), ..self })
    }

    /// Decodes the stream as [`ArchivedSound::decode`] does, for a sound copied out of its pack
    /// ([`ArchivedSound::to_sound`]): what a worker decodes while the pack's bytes stay where they are.
    pub fn decode(&self) -> Result<DecodedSound, SoundError> {
        checked(decode_vorbis(&self.vorbis)?, self.sample_rate, self.channels, self.frames)
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
        if let Some(mouth) = sound.mouth.as_ref() {
            let expected = MouthCurve::frames_for(frames, sound.sample_rate.to_native());
            if mouth.frames.len() != expected {
                return Err(SoundError::MouthLength { expected, found: mouth.frames.len() });
            }
        }
        Ok(sound)
    }
}

impl ArchivedSound {
    /// Decodes the stream, checking its format and length against what the cook found.
    /// Load-time work: see the [module docs](self).
    pub fn decode(&self) -> Result<DecodedSound, SoundError> {
        let (sample_rate, channels, frames) =
            (self.sample_rate.to_native(), self.channels.to_native(), self.frames.to_native());
        checked(decode_vorbis(&self.vorbis)?, sample_rate, channels, frames)
    }

    /// The sound, out of its pack: a copy of its stream to decode elsewhere ([`Sound::decode`]).
    #[must_use]
    pub fn to_sound(&self) -> Sound {
        Sound {
            sample_rate: self.sample_rate.to_native(),
            channels: self.channels.to_native(),
            frames: self.frames.to_native(),
            vorbis: self.vorbis.to_vec(),
            mouth: self.mouth.as_ref().map(ArchivedMouthCurve::to_curve),
        }
    }
}

/// `decoded`, if its format and length are what the cook found.
fn checked(
    decoded: DecodedSound,
    sample_rate: u32,
    channels: u32,
    frames: u32,
) -> Result<DecodedSound, SoundError> {
    if decoded.sample_rate != sample_rate {
        return Err(SoundError::SampleRateMismatch { expected: sample_rate, found: decoded.sample_rate });
    }
    if decoded.channels != channels {
        return Err(SoundError::ChannelsMismatch { expected: channels, found: decoded.channels });
    }
    if decoded.frames() as u64 != u64::from(frames) {
        return Err(SoundError::Length { expected: frames, found: decoded.frames() as u64 });
    }
    Ok(decoded)
}

/// Decodes a whole Ogg Vorbis file.
pub fn decode_vorbis(vorbis: &[u8]) -> Result<DecodedSound, SoundError> {
    let mut samples = Vec::new();
    let format = walk_vorbis(vorbis, |packet| samples.extend_from_slice(packet))?;
    Ok(DecodedSound { sample_rate: format.sample_rate, channels: format.channels, samples })
}

struct DecodedFormat {
    sample_rate: u32,
    channels: u32,
    frames: u32,
}

/// Cook and runtime validate every decoded packet together. The cook needs only the count; it
/// must not retain the whole PCM buffer just to discard it after learning the stream's format.
fn walk_vorbis(vorbis: &[u8], mut consume: impl FnMut(&[f32])) -> Result<DecodedFormat, SoundError> {
    let decode_error = |error: lewton::VorbisError| SoundError::Decode(error.to_string());
    let mut remaining = vorbis;
    let mut serials = HashSet::new();
    let mut format = None;
    let mut frames = 0_u64;
    // Decode each logical stream as a standalone file. Lewton 0.10.2's chaining path seeds its
    // granule counter from the first audio packet's page end, even before that page has ended.
    // A short, single-audio-page chain then loses its final packet. Independent readers preserve
    // precisely the same samples as decoding the component files, including their end trimming.
    loop {
        let (stream, rest, serial) = logical_stream(remaining)?;
        if !serials.insert(serial) {
            return Err(SoundError::Decode("chained Ogg streams reuse a serial number".into()));
        }
        let mut reader = OggStreamReader::new(Cursor::new(stream)).map_err(decode_error)?;
        let channels = u32::from(reader.ident_hdr.audio_channels);
        let sample_rate = reader.ident_hdr.audio_sample_rate;
        if let Some((expected_rate, expected_channels)) = format {
            if channels != expected_channels {
                return Err(SoundError::ChannelsChange { expected: expected_channels, found: channels });
            }
            if sample_rate != expected_rate {
                return Err(SoundError::SampleRateChange { expected: expected_rate, found: sample_rate });
            }
        } else {
            check_format(sample_rate, channels)?;
            format = Some((sample_rate, channels));
        }
        while let Some(packet) =
            reader.read_dec_packet_generic::<InterleavedSamples<f32>>().map_err(decode_error)?
        {
            if packet.channel_count as u32 != channels {
                return Err(SoundError::ChannelsChange {
                    expected: channels,
                    found: packet.channel_count as u32,
                });
            }
            frames += packet.samples.len() as u64 / u64::from(channels);
            if frames > u64::from(MAX_SOUND_FRAMES) {
                return Err(SoundError::TooLong { frames });
            }
            consume(&packet.samples);
        }
        remaining = rest;
        if remaining.is_empty() {
            break;
        }
    }
    if frames == 0 {
        return Err(SoundError::Silent);
    }
    let (sample_rate, channels) = format.expect("a nonempty decoded stream has a format");
    Ok(DecodedFormat { sample_rate, channels, frames: frames as u32 })
}

/// Finds a complete, unmultiplexed logical stream using Ogg page framing only. Lewton still
/// validates every page's checksum, Vorbis header and audio packet. Exact page boundaries keep
/// capture-pattern bytes in packet payloads from being mistaken for a new stream. The cooked
/// format holds pure Vorbis audio; interleaved logical streams are outside that contract.
fn logical_stream(bytes: &[u8]) -> Result<(&[u8], &[u8], u32), SoundError> {
    let invalid = |message: &str| SoundError::Decode(message.into());
    let mut offset = 0;
    let mut serial = None;
    let mut sequence = 0_u32;
    loop {
        let header = bytes
            .get(offset..)
            .and_then(|tail| tail.get(..27))
            .ok_or_else(|| invalid("missing or truncated Ogg page header or end-of-stream page"))?;
        if &header[..4] != b"OggS" {
            return Err(invalid("expected an Ogg page capture pattern"));
        }
        let page_serial = u32::from_le_bytes(header[14..18].try_into().expect("four header bytes"));
        let page_sequence = u32::from_le_bytes(header[18..22].try_into().expect("four header bytes"));
        if let Some(serial) = serial {
            if page_serial != serial || header[5] & 2 != 0 {
                return Err(invalid("multiplexed Ogg streams are not a single sound"));
            }
        } else {
            if header[5] & 2 == 0 {
                return Err(invalid("the first Ogg page must begin a logical stream"));
            }
            serial = Some(page_serial);
        }
        if page_sequence != sequence {
            return Err(invalid("Ogg pages are out of sequence"));
        }
        sequence = sequence.wrapping_add(1);
        let table_start = offset + 27;
        let body_start = table_start + usize::from(header[26]);
        let table =
            bytes.get(table_start..body_start).ok_or_else(|| invalid("truncated Ogg lacing table"))?;
        let body_len: usize = table.iter().map(|&length| usize::from(length)).sum();
        let end = body_start + body_len;
        if end > bytes.len() {
            return Err(invalid("truncated Ogg page body"));
        }
        if header[5] & 4 != 0 {
            return Ok((&bytes[..end], &bytes[end..], page_serial));
        }
        offset = end;
    }
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
    fn chained_streams_must_keep_one_format() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/third_party");
        let bell = fs::read(content.join("kenney_impact_sounds/Audio/impactBell_heavy_001.ogg")).unwrap();
        let pot = fs::read(content.join("kenney_rpg_audio/Audio/metalPot1.ogg")).unwrap();
        let mono = fs::read(content.join("kenney_interface_sounds/Audio/select_006.ogg")).unwrap();
        for (first, second, expected, found) in [(&bell, &pot, 44_100, 48_000), (&pot, &bell, 48_000, 44_100)]
        {
            let chain = [first.as_slice(), second.as_slice()].concat();
            assert!(
                matches!(decode_vorbis(&chain), Err(SoundError::SampleRateChange { expected: a, found: b }) if (a,b) == (expected,found))
            );
            assert!(matches!(Sound::from_vorbis(chain), Err(SoundError::SampleRateChange { .. })));
        }
        let chain = [bell.as_slice(), mono.as_slice()].concat();
        assert!(matches!(decode_vorbis(&chain), Err(SoundError::ChannelsChange { expected: 2, found: 1 })));
    }

    #[test]
    fn same_format_chains_preserve_every_component_sample() {
        let content = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../content/third_party/kenney_impact_sounds/Audio");
        let bell = fs::read(content.join("impactBell_heavy_001.ogg")).unwrap();
        let footstep = fs::read(content.join("footstep_carpet_000.ogg")).unwrap();
        for parts in [[&bell, &footstep], [&footstep, &bell]] {
            let chain = [parts[0].as_slice(), parts[1].as_slice()].concat();
            let expected: Vec<_> = parts
                .iter()
                .flat_map(|part| decode_vorbis(part).unwrap().samples)
                .map(f32::to_bits)
                .collect();
            let decoded = decode_vorbis(&chain).unwrap();
            assert_eq!(
                decoded.samples.iter().copied().map(f32::to_bits).collect::<Vec<_>>(),
                expected,
                "chain equals independent component decoding, bit for bit"
            );
            let sound = Sound::from_vorbis(chain).unwrap();
            assert_eq!(sound.frames as usize, decoded.frames());
            assert_eq!(Sound::read(&sound.to_cooked().unwrap()).unwrap().decode().unwrap(), decoded);
        }
    }

    #[test]
    fn an_empty_logical_component_does_not_make_a_nonempty_sound_silent() {
        let whole = sample();
        // Keep this fixture's original two header pages and its first (priming) audio packet.
        // That packet produces zero PCM frames; EOS granule zero makes a complete empty stream.
        let page_end = |bytes: &[u8], start: usize| {
            let table_end = start + 27 + usize::from(bytes[start + 26]);
            table_end + bytes[start + 27..table_end].iter().map(|&length| usize::from(length)).sum::<usize>()
        };
        let audio_start = page_end(&whole, page_end(&whole, 0));
        let table_end = audio_start + 27 + usize::from(whole[audio_start + 26]);
        let table = &whole[audio_start + 27..table_end];
        let segments = table.iter().position(|&length| length < 255).unwrap() + 1;
        let packet_len: usize = table[..segments].iter().map(|&length| usize::from(length)).sum();
        let mut empty = whole[..audio_start + 27].to_vec();
        empty[audio_start + 5] = 4; // EOS, no continued packet
        empty[audio_start + 6..audio_start + 14].copy_from_slice(&0_u64.to_le_bytes());
        empty[audio_start + 26] = segments as u8;
        empty.extend_from_slice(&table[..segments]);
        empty.extend_from_slice(&whole[table_end..table_end + packet_len]);
        let mut start = 0;
        while start < empty.len() {
            let end = page_end(&empty, start);
            empty[start + 14..start + 18].copy_from_slice(&0x5350_4645_u32.to_le_bytes());
            empty[start + 22..start + 26].fill(0);
            // Ogg's non-reflected CRC-32 polynomial, over the full page with checksum zeroed.
            let mut crc = 0_u32;
            for &byte in &empty[start..end] {
                crc ^= u32::from(byte) << 24;
                for _ in 0..8 {
                    crc = (crc << 1) ^ if crc & 0x8000_0000 != 0 { 0x04c1_1db7 } else { 0 };
                }
            }
            empty[start + 22..start + 26].copy_from_slice(&crc.to_le_bytes());
            start = end;
        }
        // Establish that the complete component reaches the decoder and really emits no PCM;
        // an invalid header, page or checksum must not be confused with the aggregate policy.
        let mut reader = OggStreamReader::new(Cursor::new(&empty)).unwrap();
        let priming = reader.read_dec_packet_generic::<InterleavedSamples<f32>>().unwrap().unwrap();
        assert!(priming.samples.is_empty());
        assert!(reader.read_dec_packet_generic::<InterleavedSamples<f32>>().unwrap().is_none());
        assert!(matches!(decode_vorbis(&empty), Err(SoundError::Silent)));
        assert!(matches!(Sound::from_vorbis(empty.clone()), Err(SoundError::Silent)));
        let expected = decode_vorbis(&whole).unwrap();
        for parts in [[whole.as_slice(), empty.as_slice()], [empty.as_slice(), whole.as_slice()]] {
            let chain = parts.concat();
            let decoded = decode_vorbis(&chain).unwrap();
            assert_eq!(decoded, expected);
            let cooked = Sound::from_vorbis(chain).unwrap().to_cooked().unwrap();
            assert_eq!(Sound::read(&cooked).unwrap().decode().unwrap(), expected);
        }
    }

    #[test]
    fn logical_stream_boundaries_do_not_hide_invalid_bytes() {
        let whole = sample();
        let (stream, rest, _) = logical_stream(&whole).unwrap();
        assert_eq!(stream.len(), whole.len());
        assert!(rest.is_empty());
        let mut trailing = whole.clone();
        trailing.extend_from_slice(b"garbage");
        assert!(decode_vorbis(&trailing).is_err());
        let duplicate = [whole.as_slice(), whole.as_slice()].concat();
        assert!(decode_vorbis(&duplicate).is_err(), "serials must be unique across a chain");
        let mut corrupt = whole.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 1;
        assert!(decode_vorbis(&corrupt).is_err(), "the decoder must still validate CRCs");
        for len in [0, 4, 26, 27, whole.len() - 1] {
            assert!(decode_vorbis(&whole[..len]).is_err());
        }
        // A second logical stream must be complete too; no silent truncation at a valid prefix.
        let mut truncated_chain = whole;
        truncated_chain.extend_from_slice(&sample()[..64]);
        assert!(decode_vorbis(&truncated_chain).is_err());
    }

    #[test]
    fn framing_requires_complete_sequential_unmultiplexed_streams() {
        // Container framing only: the decoder must still reject these pages without Vorbis
        // packets/CRCs, even though the boundary finder can delimit them.
        let page = |flags, serial: u32, sequence: u32| {
            let mut bytes = vec![0_u8; 27];
            bytes[..4].copy_from_slice(b"OggS");
            bytes[5] = flags;
            bytes[14..18].copy_from_slice(&serial.to_le_bytes());
            bytes[18..22].copy_from_slice(&sequence.to_le_bytes());
            bytes
        };
        let complete = [page(2, 1, 0), page(4, 1, 1)].concat();
        assert_eq!(logical_stream(&complete).unwrap().0, complete);
        assert!(decode_vorbis(&complete).is_err());
        for invalid in [
            page(2, 1, 0),                           // missing EOS
            [page(2, 1, 0), page(4, 2, 1)].concat(), // interleaved serial
            [page(2, 1, 0), page(4, 1, 2)].concat(), // missing page
            [page(2, 1, 0), page(6, 1, 1)].concat(), // new BOS before EOS
            page(4, 1, 0),                           // missing BOS
        ] {
            assert!(logical_stream(&invalid).is_err());
        }
    }

    #[test]
    fn counting_and_collecting_validate_the_same_packets() {
        let whole = sample();
        for bytes in [whole.as_slice(), &whole[..whole.len() / 2], b"not vorbis"] {
            let counted = Sound::from_vorbis(bytes.to_vec());
            let collected = decode_vorbis(bytes);
            match (counted, collected) {
                (Ok(sound), Ok(decoded)) => assert_eq!(
                    (sound.sample_rate, sound.channels, sound.frames as usize),
                    (decoded.sample_rate, decoded.channels, decoded.frames()),
                ),
                (Err(counted), Err(collected)) => assert_eq!(counted.to_string(), collected.to_string()),
                other => panic!("count and collect disagree: {other:?}"),
            }
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
        let wrong_rate = cooked(|sound| sound.sample_rate += 1);
        assert!(matches!(
            Sound::read(&wrong_rate).unwrap().decode(),
            Err(SoundError::SampleRateMismatch { .. })
        ));
        let wrong_channels = cooked(|sound| sound.channels = 3 - sound.channels);
        assert!(matches!(
            Sound::read(&wrong_channels).unwrap().decode(),
            Err(SoundError::ChannelsMismatch { .. })
        ));
        // The numbers pass but the stream is shorter than they say.
        let short = cooked(|sound| sound.frames += 1);
        let error = Sound::read(&short).unwrap().decode().unwrap_err();
        assert!(matches!(error, SoundError::Length { .. }));
        // A voice's curve comes back out of its pack, and one a frame short is refused.
        let voiced = good.clone().with_mouth().unwrap();
        assert_eq!(Sound::read(&voiced.to_cooked().unwrap()).unwrap().to_sound(), voiced);
        let mut curve = voiced.mouth.clone().unwrap();
        curve.frames.pop();
        let cut = Sound { mouth: Some(curve), ..voiced }.to_cooked().unwrap();
        assert!(matches!(Sound::read(&cut), Err(SoundError::MouthLength { .. })));
    }
}
