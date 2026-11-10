//! Placeholder sounds (M3 A1, specs since M4 R4): music stems and voice lines synthesized from a spec
//! each and encoded to Ogg Vorbis, so the engine has stems to fade by phase and lines to route by
//! language with no recording and no download. Own work, under the workspace's license.
//!
//! A sound starts as a spec, `<name>.sound.ron`, kept in `tools/sounds/`. `sapphire_soundgen --specs
//! tools/sounds --out content` writes each spec's `.ogg` to the `path` it names under the content
//! directory, and it's committed there: the cook reads it through its sidecar as it reads a recorded
//! sound, and never this crate. So a new placeholder sound is a spec, its row in the `SOURCE.md` beside
//! it, and whatever document names it (a score's stem, a voice line's cue), with no Rust.
//!
//! # A spec
//!
//! ```ron
//! (
//!     type: "sapphire::soundgen::Sound",
//!     version: 1,
//!     data: (
//!         path: "audio/generated/music/lantern_toll.ogg",
//!         sound: Stem((
//!             tempo_bpm: 120.0,
//!             beats_per_bar: 4,
//!             bars: 4,
//!             part: Toll((notes: [45, 41, 48, 43])),
//!         )),
//!     ),
//! )
//! ```
//!
//! - **A stem** ([`Stem`]) is a loop of whole bars at its score's tempo, beats a bar and bars, which the
//!   cook holds it to, to the frame. Its `part` is one instrument and what it plays, in MIDI's notes
//!   ([`Part`]): `Pad` (a chord a bar), `Pulse` (a bass note a bar, plucked `per_beat` times a beat),
//!   `Drums` (kick, snare and hat beats, and a `fill` in the last bar), `Bells` (a note or a rest each
//!   beat) or `Toll` (a great bell's strike note a bar). What rings past the loop's end comes round at
//!   its start, so it loops with no seam but the codec's.
//! - **A line** ([`Line`]) is a voice's shape with no words: the speaker's pitch in hertz, and
//!   syllables, each a vowel (`A`, `E`, `I`, `O` or `U`, by its first two formants), how long it's
//!   held, its pitch over the speaker's, and whether a consonant's burst leads it in. English lines are
//!   timed by stress and Japanese ones by even morae with a falling pitch accent, so the two languages
//!   sound apart. No system text-to-speech voice is used: their licences don't allow redistribution.
//!
//! How each instrument and the voice sound (harmonics, envelopes, the drum kit, a bell's partials,
//! formants) is code; what they play is the spec's. Every struct denies unknown fields, and a value
//! that can't be played (a chord a bar short, a note past 127, a tempo whose beat isn't whole frames,
//! a seed of 0) fails naming its file and field. Every sound is normalized to the same peak and
//! encoded mono at [`SAMPLE_RATE`].
//!
//! # Determinism
//!
//! Everything is a pure function of its spec. Noise comes from a seeded xorshift, the Ogg stream
//! serial is a hash of the output's path, and libvorbis has no other input, so a run makes the same
//! bytes as the last on the same machine. The instruments use `std`'s `sin`, `exp` and `powf`, and
//! libvorbis is C built by `cc`, so another OS or architecture can differ in the last bits and encode
//! differently: the committed files were made on macOS on arm64, and the tests hold them to the byte
//! there and by their decoded samples elsewhere. Moving the instruments to `sapphire_math::det` would
//! move every committed file.
//!
//! # Does not own
//!
//! - The documents that name these sounds (`.score.ron`, `.voice.ron` and the lines' `.cue.ron`
//!   beside them), which are written by hand, nor their sidecars, which the first cook makes.

pub mod spec;
mod synth;

use std::num::{NonZeroU8, NonZeroU32};
use std::path::{Path, PathBuf};

use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};

pub use spec::{
    Bells, Drums, Line, Pad, Part, Pulse, SPEC_EXTENSION, Sound, SpecError, Stem, Syllable, Synth, Toll,
    Vowel,
};

/// Every file's sample rate: plenty for placeholders, and a quarter less to decode than 32 kHz.
pub const SAMPLE_RATE: u32 = 24_000;

/// Libvorbis's quality, from -0.2 to 1: about 40 kbit/s for these.
const QUALITY: f32 = 0.3;
/// Every stem's and line's loudest sample.
const PEAK: f32 = 0.7;

/// One file: where it goes under `content/`, and its samples, mono.
pub struct Generated {
    pub path: String,
    pub samples: Vec<f32>,
    /// Whether it's a stem, whose length is its bars'.
    pub looped: bool,
}

/// A spec's file, its samples made and normalized.
pub fn generate(sound: &Sound) -> Generated {
    let (samples, looped) = match &sound.sound {
        Synth::Stem(stem) => (synth::stem(stem), true),
        Synth::Line(line) => (synth::line(line), false),
    };
    Generated { path: sound.path.clone(), samples: normalized(samples), looped }
}

/// Every spec in `dir`, read and checked, with its file, by file name. Two specs writing one path is an
/// error naming both.
pub fn read_all(dir: &Path) -> Result<Vec<(PathBuf, Sound)>, SpecError> {
    let mut sounds: Vec<(PathBuf, Sound)> = Vec::new();
    for path in spec::specs(dir)? {
        let sound = spec::read(&path)?;
        if let Some((earlier, _)) = sounds.iter().find(|(_, earlier)| earlier.path == sound.path) {
            return Err(SpecError::Field {
                path,
                field: "path".to_owned(),
                problem: format!("{:?} is written by {} too", sound.path, earlier.display()),
            });
        }
        sounds.push((path, sound));
    }
    Ok(sounds)
}

impl Generated {
    /// The file's bytes, its stream serial [`serial_of`] its path. The decoder the engine uses (lewton)
    /// doesn't trim the last block to the stream's end, so a sound can come back a block longer than it
    /// went in: a line is given that much quiet at its end and encoded again, until it comes back as
    /// long as it is. A stem can't be: it must stay its bars, and this fails instead.
    pub fn encode(&mut self) -> anyhow::Result<Vec<u8>> {
        for _ in 0..4 {
            let bytes = encode(&self.samples, serial_of(&self.path))?;
            let frames = sapphire_assets::sound::decode_vorbis(&bytes)?.frames();
            if frames == self.samples.len() {
                return Ok(bytes);
            }
            anyhow::ensure!(
                !self.looped,
                "{} decodes to {frames} frames, not its {} frames of bars",
                self.path,
                self.samples.len()
            );
            self.samples.resize(frames, 0.0);
        }
        anyhow::bail!("{} never decodes to its own length", self.path)
    }
}

/// `samples` as an Ogg Vorbis stream, mono at [`SAMPLE_RATE`], with the stream serial `serial`.
pub fn encode(samples: &[f32], serial: i32) -> anyhow::Result<Vec<u8>> {
    let (Some(rate), Some(mono)) = (NonZeroU32::new(SAMPLE_RATE), NonZeroU8::new(1)) else {
        unreachable!("neither is zero")
    };
    let mut builder = VorbisEncoderBuilder::new_with_serial(rate, mono, Vec::new(), serial);
    builder
        .bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr { target_quality: QUALITY });
    let mut encoder = builder.build()?;
    for block in samples.chunks(4_096) {
        encoder.encode_audio_block([block])?;
    }
    Ok(encoder.finish()?)
}

/// A stable serial for the file at `path`: FNV-1a, folded to 31 bits.
pub fn serial_of(path: &str) -> i32 {
    let hash = path.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    (hash & 0x7fff_ffff) as i32
}

fn normalized(mut samples: Vec<f32>) -> Vec<f32> {
    let peak = samples.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak > 0.0 {
        for sample in &mut samples {
            *sample *= PEAK / peak;
        }
    }
    samples
}

#[cfg(test)]
mod tests;
