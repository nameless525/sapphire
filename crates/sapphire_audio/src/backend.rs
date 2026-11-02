//! The [`AudioBackend`] trait: what the rest of the crate asks of a mixer.
//!
//! It is deliberately small and in plain numbers. The decisions (which sound, how loud at this
//! distance, what a snapshot does to a voice) are made above it, by
//! [`AudioSystem`](crate::AudioSystem), so game logic is tested against
//! [`MockBackend`](crate::MockBackend) with no device, and another mixer (FMOD in a private game
//! project, m2-plan §3) needs only these calls.

use std::fmt;

use sapphire_assets::AssetId;
use sapphire_assets::cue::Bus;
use sapphire_assets::sound::DecodedSound;

/// A voice's place in the backend: an index below [`MAX_VOICES`](crate::MAX_VOICES), reused once
/// [`AudioBackend::voice_finished`] has said so.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VoiceSlot(pub u32);

/// What a voice sounds like right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParams {
    /// Gain in decibels. [`SILENCE_DB`](sapphire_assets::cue::SILENCE_DB) and below is silent.
    pub gain_db: f32,
    /// Left (-1) to right (1).
    pub pan: f32,
    /// Playback rate: 1 is the sound as authored.
    pub pitch: f32,
}

/// What a bus does to everything on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusParams {
    /// The low-pass filter's cutoff in hertz. [`Self::OPEN`] takes the filter out.
    pub low_pass_hz: f32,
}

impl BusParams {
    /// The filter out of the signal.
    pub const OPEN: Self = Self { low_pass_hz: f32::INFINITY };
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

    /// Starts `sound` on `bus` in a free slot, frozen if `frozen`.
    fn start_voice(
        &mut self,
        slot: VoiceSlot,
        sound: AssetId,
        bus: Bus,
        params: VoiceParams,
        frozen: bool,
    ) -> Result<(), StartError>;

    /// Moves a live voice to `params`, smoothly enough not to click.
    fn set_voice(&mut self, slot: VoiceSlot, params: VoiceParams);

    /// Stops a live voice where it is, or lets it go on from there.
    fn freeze_voice(&mut self, slot: VoiceSlot, frozen: bool);

    /// Whether the voice in `slot` has played out, which frees the slot.
    fn voice_finished(&mut self, slot: VoiceSlot) -> bool;

    /// Sets what a bus does to its voices, smoothly enough not to click.
    fn set_bus(&mut self, bus: Bus, params: BusParams);

    /// Stops asking the device for audio, keeping every voice where it is (spec §6.5).
    fn suspend(&mut self);

    /// Takes up where [`Self::suspend`] left off.
    fn resume(&mut self);

    /// What the output has done.
    fn status(&mut self) -> DeviceStatus;
}
