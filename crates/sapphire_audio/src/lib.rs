//! Audio (spec §5.15, layer L3): cues in, sound out, with the mix in step with the game's clocks.
//!
//! ```text
//! AudioFrame (plain data, once a frame): listener, emitters, world time scale, snapshots asked
//!     │                                    for, cue and line requests, the score and its phase
//! AudioSystem: picks variation, attenuates and pans, moves the mix's snapshots and ducks, drives
//!     │        hitstop, routes lines by the voice language, fades stems on the bar
//!     │  AudioBackend: plain numbers per voice and bus, and the mix's buses made once
//!     ├── KiraBackend<CpalOutput>: kira's mixer on the audio device, each voice the engine's own
//!     ├── KiraBackend<OfflineOutput>: the same mixer, rendered by a test with no device
//!     └── MockBackend: writes down what was asked
//! ```
//!
//! # Owns
//!
//! - The [`AudioBackend`] trait, kira's backend and the mock, and what pulls kira's audio
//!   ([`CpalOutput`], [`OfflineOutput`]).
//! - **The voice** (M6 U1): every sound plays as the engine's own sound on kira's public `Sound` trait,
//!   holding its samples in their authored channels as 16-bit integers where they fit, with its own
//!   low-pass, and counted in [`AudioStats::decoded_bytes`].
//! - **Cue playback** ([`AudioSystem`]): a [`Cue`](sapphire_assets::cue::Cue)'s variation, picked
//!   from randomness the caller owns ([`CueRandom`], [`CueRng`]), and voices placed by distance and
//!   side from the [`Listener`], following their [`Emitter`] by [`EmitterId`].
//! - **The cue's tree** (M6 U4a): each start walks its [`CueNode`](sapphire_assets::cue::CueNode)s, random, sequence,
//!   layer, switch and blend, laid out flat as the cue is added, reading the [`CueParameters`] it was asked with,
//!   and draws the same count of numbers whatever the tree; the sounds it reaches are its voices, counted once
//!   toward a limit.
//! - **The mix** (M6 U2): a [`Mix`](sapphire_assets::mix::Mix) document's bus tree under the five categories,
//!   each bus's effects and reverb sends, made once on the mixer ([`AudioSystem::set_mix`]); its snapshots,
//!   offsets to buses moved by attack, release and priority, asked for by name ([`AudioFrame::snapshots`],
//!   [`PAUSE_SNAPSHOT`]) or following the world clock, as the perfect dodge's slow motion does; its ducking
//!   rules, following the cooked mouth curve of each voice line at its playhead; and its output profiles
//!   ([`AudioSystem::set_output_profile`]). With no mix given it's [`Mix::plain`](sapphire_assets::mix::Mix::plain).
//! - **Voice limits** (M6 U3): each voice ranked by its cue's priority and how loud it is at the listener;
//!   a cue's own [`Limit`](sapphire_assets::cue::Limit) and its mix voice group's, by Unreal's rules; the
//!   quality tier's [`VoiceCaps`] on voices mixed and kept; and virtual voices, kept unmixed with their
//!   playhead counted on, that come back where they would be, fading in over [`VIRTUAL_FADE`].
//! - **Clock-aware audio:** snapshots driven by the world time scale, and each actor's hitstop, which ducks or
//!   freezes the voices attached to it as their cue says. The inputs are plain numbers per frame.
//! - **Music** (M3 A1): a [`Score`](sapphire_assets::score::Score)'s stems loop together on the
//!   music bus from one kira clock, and a phase asked for by name ([`MusicRequest`]) fades the stems
//!   it changes on the clock's next bar line, at least [`LEAD_SECONDS`] ahead, in the mixer's own time.
//! - **Voice lines** (M3 A1): [`VoiceRoute`] holds each language's
//!   [`VoiceLines`](sapphire_assets::voice::VoiceLines) and answers a [`LineKey`] in the voice
//!   language the player chose, which is not the text's; the mix's ducking rules take the music down under it.
//!   Each update says which lines it started and how long each plays ([`AudioSystem::heard_lines`], M6 U14),
//!   which is what a caption lasts.
//! - **A conversation's line** (M5 C6): one [`SpeechRequest`] at a time, started, held by a pause and cut
//!   by the next or by none, with its playhead counted here in the real seconds its voice has played
//!   ([`AudioSystem::speaking`]), so a face can read the sound's mouth curve at it without asking the
//!   audio thread anything.
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
//! A warmed cue allocates a little on the calling thread, all of it a voice's command buffers and kira's
//! box for it: see
//! `tests/allocations.rs`, which holds it to a bound, and holds the mixing path to none. Cues fired
//! from a fixed step (rule R6) should be queued for the frame's update rather than played there.

mod backend;
mod events;
mod kira_backend;
pub mod loudness;
mod master;
mod mix;
mod mock;
mod music;
mod output;
mod rng;
mod room_reverb;
mod rooms;
mod system;
mod voice;
mod voice_sound;

pub use backend::{
    AudioBackend, BusId, BusParams, DeviceState, DeviceStatus, STOP_FADE, StartError, StemStart,
    VIRTUAL_FADE, VoiceParams, VoiceSlot, VoiceStart,
};
pub use events::{CueParameters, MAX_CUE_PARAMETERS, ParameterValue, SURFACE};
pub use kira_backend::KiraBackend;
pub use master::{DynamicRange, LIMITER_CEILING_DB, LIMITER_LOOK_AHEAD, LIMITER_RELEASE, MAX_RATE, Master};
pub use mock::{MockBackend, MockCall};
pub use music::{LEAD_SECONDS, MusicRequest};
pub use output::{CpalOutput, OfflineOutput, Output, device_opens};
pub use rng::{CueRandom, CueRng};
pub use rooms::{Room, RoomShape, Rooms, Space};
pub use system::{
    AudioFrame, AudioStats, AudioSystem, CueRequest, CueTarget, Emitter, EmitterId, HeardLine, LineRequest,
    Listener, MAX_VIRTUAL_VOICES, MAX_VOICES, PAUSE_SNAPSHOT, SHORTEST_VIRTUAL_SECONDS, Speaking,
    SpeechRequest, VoiceCaps, Volumes,
};
pub use voice::{LineKey, VoiceRoute, VoiceRouteError};

/// `tracing` target for everything this crate logs.
const LOG_TARGET: &str = "sapphire::audio";
