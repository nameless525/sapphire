//! The [`AudioBackend`] trait: what the rest of the crate asks of a mixer.
//!
//! It is deliberately small and in plain numbers. The decisions (which sound, how loud at this
//! distance, what a snapshot does to a voice) are made above it, by
//! [`AudioSystem`](crate::AudioSystem), so game logic is tested against
//! [`MockBackend`](crate::MockBackend) with no device, and another mixer (FMOD in a private game
//! project, m2-plan §3) needs only these calls.

use std::fmt;
use std::time::Duration;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::mix::{Mix, OutputProfile};
use sapphire_assets::sound::DecodedSound;

/// A voice's place in the backend: an index below [`MAX_VOICES`](crate::MAX_VOICES), reused once
/// [`AudioBackend::voice_finished`] has said so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoiceSlot(pub u32);

/// A bus of the mix (M6 U2): its place in the [`Mix`]'s buses, the five categories first, in [`Bus::ALL`]'s order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BusId(pub u16);

impl BusId {
    /// A category's bus, which every mix has in the same place.
    pub const fn category(bus: Bus) -> Self {
        Self(bus.index() as u16)
    }

    pub const fn index(self) -> usize {
        self.0 as usize
    }
}

/// What a voice sounds like right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParams {
    /// Gain in decibels. [`SILENCE_DB`](sapphire_assets::cue::SILENCE_DB) and below is silent.
    pub gain_db: f32,
    /// Left (-1) to right (1).
    pub pan: f32,
    /// Playback rate: 1 is the sound as authored.
    pub pitch: f32,
    /// The voice's own low-pass cutoff in hertz (M6 U1): one pole, so 6 dB an octave. Any value that isn't
    /// finite, [`f32::INFINITY`] as [`Self::NEUTRAL`] has it, takes the filter out.
    pub low_pass_hz: f32,
}

impl VoiceParams {
    /// The sound as authored: full gain, centred, at its own pitch, unfiltered.
    pub const NEUTRAL: Self = Self { gain_db: 0.0, pan: 0.0, pitch: 1.0, low_pass_hz: f32::INFINITY };
}

/// What a snapshot's filter on a bus does to everything on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusParams {
    /// The low-pass filter's cutoff in hertz. [`Self::OPEN`] takes the filter out.
    pub low_pass_hz: f32,
}

impl BusParams {
    /// The filter out of the signal.
    pub const OPEN: Self = Self { low_pass_hz: f32::INFINITY };
}

/// One of a score's stems, as [`AudioBackend::load_music`] readies it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemStart {
    pub sound: AssetId,
    /// The gain it starts at: [`SILENCE_DB`](sapphire_assets::cue::SILENCE_DB) for a stem the phase
    /// doesn't play.
    pub gain_db: f32,
}

/// Where the output stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceState {
    /// Not started yet: asked to start, or asked for no device at all.
    #[default]
    Starting,
    /// The stream runs and the mixer's callback is called.
    Running,
    /// The app asked it to stop: the stream is paused, and voices wait where they were.
    Suspended,
    /// No device can be opened, or the one in use went away. Logged, not an error: the backend
    /// keeps trying, and the app runs on without sound.
    NoDevice,
}

impl DeviceState {
    /// Whether a new voice would be heard, now or as soon as the stream starts.
    pub const fn plays(self) -> bool {
        matches!(self, Self::Starting | Self::Running)
    }
}

impl fmt::Display for DeviceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Suspended => "suspended",
            Self::NoDevice => "no device",
        })
    }
}

/// What the output has done since it started (rule R9).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DeviceStatus {
    pub state: DeviceState,
    /// Frames per second the mixer runs at.
    pub sample_rate: u32,
    /// Channels of the stream. 0 until one has opened.
    pub channels: u16,
    /// Audio callbacks so far.
    pub callbacks: u64,
    /// Frames the callbacks asked for.
    pub frames: u64,
    /// Underruns the device reported.
    pub xruns: u64,
    /// Nanoseconds the callbacks took, all told: the mixer's own cost.
    pub mix_nanos: u64,
    /// The longest callback so far, in nanoseconds.
    pub mix_nanos_max: u64,
    /// Streams opened: more than 1 means the device was lost and came back.
    pub streams_opened: u32,
    /// Whether the device refused a stream the low-latency path it asked for, so sound plays on its
    /// normal one, tens of milliseconds later: Android's AAudio on the emulator always, and on a phone
    /// for some outputs. Not an error, and said once in the log.
    pub low_latency_refused: bool,
}

impl DeviceStatus {
    /// The share of real time the mixer takes: its callbacks' time over the audio they made.
    pub fn mix_load(&self) -> f32 {
        if self.frames == 0 || self.sample_rate == 0 {
            return 0.0;
        }
        let audio_nanos = self.frames as f64 * 1e9 / f64::from(self.sample_rate);
        (self.mix_nanos as f64 / audio_nanos) as f32
    }
}

/// Why a voice didn't start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartError {
    /// No [`AudioBackend::add_sound`] for that sound.
    UnknownSound(AssetId),
    /// The mixer has no room for another sound.
    MixerFull,
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSound(id) => write!(f, "sound {id} isn't loaded"),
            Self::MixerFull => f.write_str("the mixer has no room for another sound"),
        }
    }
}

/// A mixer. See the [crate docs](crate).
///
/// Every call but [`Self::add_sound`] is made during a frame, so none may wait on the device: the
/// audio callback must never be blocked by them or block them.
pub trait AudioBackend {
    /// Makes a decoded sound playable. Load-time work.
    fn add_sound(&mut self, id: AssetId, sound: &DecodedSound);

    /// Forgets a sound. Voices already playing it go on.
    fn remove_sound(&mut self, id: AssetId);

    /// Makes `mix`'s buses, sends and profiles (M6 U2), in place of the last mix's: before any voice plays or
    /// music loads, since what plays on the last mix's buses ends. Load-time work: the mixer allocates for each
    /// bus. Each bus starts at its own volume, open, unpaused, under the `Home` profile. A new backend has
    /// [`Mix::plain`]'s.
    fn set_mix(&mut self, mix: &Mix);

    /// Starts `sound` on `bus` in a free slot, frozen if `frozen`.
    fn start_voice(
        &mut self,
        slot: VoiceSlot,
        sound: AssetId,
        bus: BusId,
        params: VoiceParams,
        frozen: bool,
    ) -> Result<(), StartError>;

    /// Moves a live voice to `params`, smoothly enough not to click.
    fn set_voice(&mut self, slot: VoiceSlot, params: VoiceParams);

    /// Stops a live voice where it is, or lets it go on from there.
    fn freeze_voice(&mut self, slot: VoiceSlot, frozen: bool);

    /// Ends a live voice for good, frozen or not, soon enough to read as at once and smoothly enough not
    /// to click. [`Self::voice_finished`] says when it has.
    fn stop_voice(&mut self, slot: VoiceSlot);

    /// Whether the voice in `slot` has played out, which frees the slot.
    fn voice_finished(&mut self, slot: VoiceSlot) -> bool;

    /// Moves a bus's snapshot filter, smoothly enough not to click. A bus no snapshot filters has none, and
    /// ignores it.
    fn set_bus(&mut self, bus: BusId, params: BusParams);

    /// Fades a bus's gain to `gain_db` over `fade`: its own volume with the snapshots' and the ducks' on top.
    fn set_bus_gain(&mut self, bus: BusId, gain_db: f32, fade: Duration);

    /// Pauses a bus and every bus under it, fading out over `fade`, its voices waiting where they are; or lets
    /// it go on from there, fading in.
    fn pause_bus(&mut self, bus: BusId, paused: bool, fade: Duration);

    /// Plays the whole mix through `profile`'s volume and effects, or `Home`'s if the mix has none for it,
    /// smoothly enough not to click.
    fn set_output_profile(&mut self, profile: OutputProfile);

    /// Readies `stems` to loop together on the [`Music`](Bus::Music) category's bus, on a clock that will count
    /// `tempo_bpm` beats a minute from their first frame, and ends any music playing over `fade`. They
    /// wait for [`Self::play_music`], in a later frame, so that they and the clock reach the mixer
    /// together however its callbacks fall. Made as a fight starts, not in its frames: the mixer
    /// allocates for each stem.
    fn load_music(&mut self, tempo_bpm: f32, stems: &[StemStart], fade: Duration) -> Result<(), StartError>;

    /// Starts the music [`Self::load_music`] readied, from the mixer's next buffer.
    fn play_music(&mut self);

    /// Fades the stem at `stem` in the last [`Self::load_music`] to `gain_db` over `fade`, beginning
    /// on the music clock's beat `beat`, or as soon as the command lands if that's past. A fade not yet
    /// begun is replaced.
    fn fade_stem(&mut self, stem: usize, gain_db: f32, beat: u64, fade: Duration);

    /// Fades the music out over `fade`, and stops its clock.
    fn stop_music(&mut self, fade: Duration);

    /// Beats since the music's first frame, as the mixer last counted them: `None` while no clock
    /// runs, before the first buffer and after [`Self::stop_music`].
    fn music_beats(&mut self) -> Option<f64>;

    /// Stops asking the device for audio, keeping every voice where it is (spec §6.5).
    fn suspend(&mut self);

    /// Takes up where [`Self::suspend`] left off.
    fn resume(&mut self);

    /// What the output has done.
    fn status(&mut self) -> DeviceStatus;

    /// Folds the whole mix to mono, or back to stereo (M4 A2), smoothly enough not to click.
    fn set_mono(&mut self, mono: bool);

    /// The bytes of decoded sound the backend holds for [`Self::add_sound`]'s sounds (M6 U1): what playing
    /// them costs in memory, beside their cooked bytes in the asset server's ledger.
    fn decoded_bytes(&self) -> u64;
}
