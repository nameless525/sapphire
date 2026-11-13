//! Timelines as plain data: what the cooker, the simulation and the sequencer agree on (spec §5.23).
//!
//! A timeline is an authored stretch of a game: a cutscene, or an ultimate's cinematic. This crate holds
//! the document and the cooked form and nothing that plays them, so the cooker can link it without a
//! renderer, and a replay can link it without a GPU.
//!
//! # Owns
//!
//! - **The document** ([`document`]): `.timeline.ron`, [`TimelineDocument`], and its checks, which name
//!   the field.
//! - **The cooked format** ([`cooked`]): [`Timeline`], an rkyv archive (`.stimeline`), split into what
//!   the simulation reads ([`TimelineSimulation`]) and what only presentation does
//!   ([`TimelinePresentation`]).
//! - **The digest** ([`TimelineSimulation::digest`]): what a replay pins.
//!
//! # Does not own
//!
//! - Playing the simulation's half: `sapphire_gameplay`'s `timeline` module, which runs it inside the
//!   fixed step. Playing the presentation's half: `sapphire_sequencer`, at L4.
//! - Reading sources and sidecars, cache keys and packs: `sapphire_pipeline`'s `timeline` module.
//! - Asset kinds and residency: `sapphire_assets`, which reads this crate's format as
//!   `AssetKind::Timeline`.
//!
//! # Timelines are presentation, and two things more
//!
//! A timeline's camera, characters, cues, post and subtitles never reach the simulation, so a replay
//! plays the same with them gone. Only its **time scale** and **gameplay events** do, and they're the
//! [`TimelineSimulation`]: a length in whole steps and a sorted list of [`Beat`]s. That half is all the
//! simulation holds of a timeline, so a skip has nothing else to get right.
//!
//! # The two clocks
//!
//! A timeline names one clock ([`TimelineClock`]), and every track of it reads that one playhead, so a
//! subtitle can't drift from the sound it goes with.
//!
//! - **[`TimelineClock::Real`]** is a cutscene: it holds the simulation at a step's edge and plays on
//!   real seconds, so what it does to the simulation doesn't depend on how long anyone watched. Its
//!   simulation half is **no steps long**, and every gameplay event it has fires on the first step after
//!   it. It can't have a time scale, since no step runs under one.
//! - **[`TimelineClock::Steps`]** is a cinematic in a fight: the simulation runs under it, input held,
//!   and the playhead is the steps it has taken. A time scale here is a beat on the world's clock.
//!
//! # Determinism
//!
//! What the simulation reads is parsed from decimal text, multiplied by 60 and rounded, and hashed by
//! value: IEEE's exact operations and no C math library, so it cooks to the same beats on every OS and
//! architecture. `[simulation]` in `xtask/layers.toml` holds this crate to the simulation's
//! `clippy.toml`. Presentation's floats aren't in the digest.

pub mod cooked;
pub mod document;

pub use cooked::{
    ArchivedTimeline, Beat, BeatKind, CookedCharacter, CookedCue, CookedFace, CookedPlay, MAX_BEATS,
    TIMELINE_EXTENSION, Timeline, TimelinePresentation, TimelineSimulation,
};
pub use document::{
    CameraKey, CameraTrack, CharacterTrack, CueKey, DEFAULT_FACE_BLEND, Ease, EventKey, EyeRule, FaceKey,
    LookKey, PlaceKey, PlayRange, PostParameter, PostTrack, ScalarKey, ScaleKey, SubtitleKey, TimelineClock,
    TimelineDocument, TimelineError,
};
