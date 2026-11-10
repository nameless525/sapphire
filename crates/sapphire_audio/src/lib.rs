//! Audio (spec §5.15, layer L3): cues in, sound out, with the mix in step with the game's clocks.
//!
//! ```text
//! AudioFrame (plain data, once a frame): listener, emitters, world time scale, cue and line
//!     │                                    requests, the score and its phase
//! AudioSystem: picks variation, attenuates and pans, drives the snapshot and hitstop, routes lines
//!     │        by the voice language, fades stems on the bar, ducks the music under a line
//!     │  AudioBackend: plain numbers per voice and bus
//!     ├── KiraBackend<CpalOutput>: kira's mixer on the audio device
//!     ├── KiraBackend<OfflineOutput>: the same mixer, rendered by a test with no device
//!     └── MockBackend: writes down what was asked
//! ```
//!
//! # Owns
//!
//! - The [`AudioBackend`] trait, kira's backend and the mock, and what pulls kira's audio
//!   ([`CpalOutput`], [`OfflineOutput`]).
//! - **Cue playback** ([`AudioSystem`]): a [`Cue`](sapphire_assets::cue::Cue)'s variation, picked
//!   from randomness the caller owns ([`CueRandom`], [`CueRng`]), and voices placed by distance and
//!   side from the [`Listener`], following their [`Emitter`] by [`EmitterId`].
//! - **Clock-aware audio:** the perfect-dodge snapshot ([`ClockSnapshot`]), pitch and a low-pass from
//!   the world time scale, on the world's buses; and each actor's hitstop, which ducks or freezes
//!   the voices attached to it as their cue says. The inputs are plain numbers per frame.
//! - **Music** (M3 A1): a [`Score`](sapphire_assets::score::Score)'s stems loop together on the
//!   music bus from one kira clock, and a phase asked for by name ([`MusicRequest`]) fades the stems
//!   it changes on the clock's next bar line, at least [`LEAD_SECONDS`] ahead, in the mixer's own time.
//! - **Voice lines** (M3 A1): [`VoiceRoute`] holds each language's
//!   [`VoiceLines`](sapphire_assets::voice::VoiceLines) and answers a [`LineKey`] in the voice
//!   language the player chose, which is not the text's; the music goes down by the score's
//!   `duck_db` while a line plays.
//! - **Stats** ([`AudioStats`], rule R9): the update's time against spec §3.2's budget, voices,
//!   dropped cues, lines and phase changes, and what the device has done ([`DeviceStatus`]).
//!
//! # Does not own
//!
//! - Cooked sounds and cue documents: `sapphire_assets`, as plain data, so the cooker links no
//!   mixer. Loading them: the app, through its asset server.
//! - ECS: the app or `sapphire_gameplay` fills an [`AudioFrame`] however it likes.
//! - The OS's audio session (iOS's category and interruptions, Android's context under SDL's entry):
//!   `sapphire_platform`, whose results the app hands to [`AudioSystem::set_background`] and
//!   [`AudioSystem::set_interrupted`].
//! - What phase a fight is in, and which lines a game speaks when: the caller says, by name and key.
//!   The simulation never waits on audio, and nothing here reaches it.
//!
//! # Threads and cost
//!
//! kira mixes on the device's callback, never on a job thread, and [`AudioSystem::update`] only
//! writes lock-free commands to it. Nothing here runs until an app makes a system: an app without
//! audio has no thread, no device and no cost but the code's size.
//!
//! A warmed cue allocates a little on the calling thread, all of it kira's per-sound state: see
//! `tests/allocations.rs`, which holds it to a bound, and holds the mixing path to none. Cues fired
//! from a fixed step (rule R6) should be queued for the frame's update rather than played there.

mod backend;
mod kira_backend;
mod mock;
mod music;
mod output;
mod rng;
mod snapshot;
mod system;
mod voice;

pub use backend::{
    AudioBackend, BusParams, DeviceState, DeviceStatus, StartError, StemStart, VoiceParams, VoiceSlot,
};
pub use kira_backend::KiraBackend;
pub use mock::{MockBackend, MockCall};
pub use music::{LEAD_SECONDS, MusicRequest};
pub use output::{CpalOutput, OfflineOutput, Output, device_opens};
pub use rng::{CueRandom, CueRng};
pub use snapshot::{ClockSnapshot, SnapshotMix, SnapshotTuning};
pub use system::{
    AudioFrame, AudioStats, AudioSystem, CueRequest, CueTarget, Emitter, EmitterId, LineRequest, Listener,
    MAX_VOICES, Volumes,
};
pub use voice::{LineKey, VoiceRoute, VoiceRouteError};

/// `tracing` target for everything this crate logs.
const LOG_TARGET: &str = "sapphire::audio";
