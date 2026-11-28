//! A master's encode (M6 U11): libvorbis through `vorbis_rs`, at the settings `sapphire_soundgen` encodes its
//! stems with, so a generated line imported here is the stream it was when the generator encoded it. Only
//! `vo` runs this, never a cook: libvorbis's floating point isn't the same on every machine.

use std::num::{NonZeroU8, NonZeroU32};

use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

/// Libvorbis's quality, from -0.2 to 1: about 40 kbit/s for a mono line at 24 kHz. `sapphire_soundgen`'s
/// `QUALITY` too.
pub const QUALITY: f32 = 0.3;

/// The samples a block of the encoder takes at once.
const BLOCK: usize = 4_096;

/// `samples`, one channel at `sample_rate`, as an Ogg Vorbis stream that decodes as long as it is: the
/// decoder the engine uses (lewton) doesn't trim the last block to the stream's end, so a sound can come
/// back a block longer than it went in. The line is given that much quiet at its end and encoded again,
/// until it comes back as long as it is. `serial` is the stream's serial ([`serial_of`]).
pub fn encode_line(samples: &[f32], sample_rate: u32, serial: i32) -> anyhow::Result<Vec<u8>> {
    let mut padded = samples.to_vec();
    for _ in 0..4 {
        let bytes = encode(&padded, sample_rate, serial)?;
        let frames = sapphire_assets::sound::decode_vorbis(&bytes)?.frames();
        if frames == padded.len() {
            return Ok(bytes);
        }
        // Padding only lengthens: a decode shorter than its input would have the resize cut the line's end
        // (fixed after M6 U11's review).
        if frames < padded.len() {
            anyhow::bail!("a line of {} frames decodes to {frames}, shorter than it", padded.len())
        }
        padded.resize(frames, 0.0);
    }
    anyhow::bail!("a line of {} frames never decodes to its own length", samples.len())
}

/// A stable serial for the Ogg at `path` under the content directory: FNV-1a, folded to 31 bits, as
/// `sapphire_soundgen` gives its files.
pub fn serial_of(path: &str) -> i32 {
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (hash & 0x7fff_ffff) as i32
}

/// `samples` as one Ogg Vorbis stream at [`QUALITY`].
fn encode(samples: &[f32], sample_rate: u32, serial: i32) -> anyhow::Result<Vec<u8>> {
    let (Some(rate), Some(mono)) = (NonZeroU32::new(sample_rate), NonZeroU8::new(1)) else {
        anyhow::bail!("a sound at {sample_rate} Hz")
    };
    let mut builder = VorbisEncoderBuilder::new_with_serial(rate, mono, Vec::new(), serial);
    builder
        .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: QUALITY });
    let mut encoder = builder.build()?;
    for block in samples.chunks(BLOCK) {
        encoder.encode_audio_block([block])?;
    }
    Ok(encoder.finish()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_encodes_the_same_twice_and_decodes_to_its_length() {
        // A triangle wave: no transcendentals, which the cooker's lints ban.
        let samples: Vec<f32> =
            (0..10_001).map(|n| 0.3 * ((n % 200) as f32 / 50.0 - 2.0).abs() - 0.3).collect();
        let bytes = encode_line(&samples, 24_000, serial_of("vo/en/test.ogg")).unwrap();
        assert_eq!(bytes, encode_line(&samples, 24_000, serial_of("vo/en/test.ogg")).unwrap());
        let decoded = sapphire_assets::sound::decode_vorbis(&bytes).unwrap();
        assert_eq!((decoded.sample_rate, decoded.channels), (24_000, 1));
        assert!(decoded.frames() >= samples.len());
    }
}
