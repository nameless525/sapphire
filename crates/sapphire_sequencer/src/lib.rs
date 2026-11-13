//! The timeline player (spec §5.23, `docs/m3-plan.md` §3): what a cutscene and an ultimate's cinematic
//! show, moment by moment, as plain values.
//!
//! # Owns
//!
//! - **The playhead** ([`TimelinePlayer`]): one, on the clock its timeline names, which every track reads,
//!   so a subtitle can't drift from the sound it goes with. It only goes forward.
//! - **What each track says at the playhead:** the [`Shot`], the post offsets
//!   ([`TimelinePlayer::apply_post`]), the [`Subtitle`]'s keys, each [`CharacterCue`], and the
//!   [`FiredCue`]s since the last advance. Each but the last is a pure function of the playhead, so it
//!   costs a binary search, and there's nothing to get out of step.
//! - **What a skip means to presentation** ([`TimelinePlayer::skip`]): everything drops at once, and cues it
//!   jumped never fire.
//!
//! # Does not own
//!
//! - The document and the cooked form: `sapphire_timeline`.
//! - What a timeline does to the simulation: `sapphire_gameplay`'s `timeline` module. Nothing here is
//!   read by it or moves it, so a replay plays the same with this whole crate gone, and it's out of
//!   `[simulation]` and off `sapphire_replay`'s tree.
//! - Drawing and playing any of it. The stage hands the shot to the camera director's takeover slot, the
//!   post to the frame's packet (rule R4), a cue to `sapphire_cues` and a character's row to its animation
//!   graph; the game's HUD document resolves a subtitle's keys through `sapphire_loc`. This crate has no
//!   edge to any of them but plain data.
//!
//! # A frame with a timeline
//!
//! ```ignore
//! let mut player = TimelinePlayer::new(timeline, Origin::default());
//! sim.play_timeline(id)?;
//! // Each frame, with the frame's unscaled seconds and, for a timeline on the steps, its progress:
//! player.advance(real_dt, sim.timeline().map(|run| StepClock { taken: run.tick, alpha }));
//! if !player.holds_simulation() { /* the app's fixed steps run */ }
//! if pressed_skip {
//!     let at_ms = player.skip();                // Presentation lets go at once.
//!     for _ in 0..sim.timeline_steps_held() {   // None, for a cutscene.
//!         sim.step(&TickActions::NEUTRAL);      // The steps watching would have taken.
//!     }
//! }
//! ```
//!
//! A timeline's camera hands back past its end with the simulation already let go, so a fight underneath is
//! live when the view reaches it: the director's weights do that, with their spring, on the unscaled clock.

mod player;

pub use player::{
    CameraTerms, CharacterCue, FaceCue, FiredCue, Origin, Placed, Playing, SequencerStats, Shot, StepClock,
    Subtitle, TimelinePlayer,
};
