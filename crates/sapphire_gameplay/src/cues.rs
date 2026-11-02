//! The cue queue: what a step tells presentation happened (spec §5.22, step 8, rule R3).
//!
//! Moves publish here and nothing in the simulation reads it back. M2 E3 drains it into effects,
//! sound, shake and rumble, which is how the simulation stays headless. It holds one step's events:
//! read it after each [`crate::Simulation::step`], as an app running two steps a frame does twice.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_time::{SimDuration, SimTime};

use crate::actors::ActorId;

/// Most cues one step can publish. A step's are a handful per actor, so this is room for a busy
/// fight's; past it they're counted and dropped ([`crate::SimStats::cues_dropped`]), since losing a
/// sound is better than failing a fight.
pub const CUE_QUEUE_LEN: usize = 64;

/// A presentation event, with exactly when it happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CueEvent {
    pub actor: ActorId,
    /// What it is: E3's table says what that means.
    pub cue: Name,
    /// The world's tick on the step that published it.
    pub tick: u64,
    /// When on the actor's own clock, in exact sub-ticks: under a time scale, this can lie anywhere
    /// in the step, and a big step can publish several, in the order they happened.
    pub local_time: SimTime,
    /// How far into its move.
    pub move_time: SimDuration,
}

/// One step's cues, in the order they fired, each actor's in time order. Nothing in the simulation
/// reads it back.
#[derive(Resource, Debug)]
pub struct CueQueue {
    events: [Option<CueEvent>; CUE_QUEUE_LEN],
    len: usize,
    published: u64,
    dropped: u64,
}

impl Default for CueQueue {
    fn default() -> Self {
        Self { events: [None; CUE_QUEUE_LEN], len: 0, published: 0, dropped: 0 }
    }
}

impl CueQueue {
    /// Publishes a cue, or counts it if this step has no room left.
    pub fn push(&mut self, event: CueEvent) {
        match self.events.get_mut(self.len) {
            Some(slot) => {
                *slot = Some(event);
                self.len += 1;
                self.published += 1;
            }
            None => self.dropped += 1,
        }
    }

    /// This step's cues.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = CueEvent> + '_ {
        self.events[..self.len].iter().map(|event| event.expect("the first `len` are filled"))
    }

    /// How many this step published.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Whether this step published none.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Cues published so far.
    #[must_use]
    pub const fn published(&self) -> u64 {
        self.published
    }

    /// Cues dropped so far, for want of room.
    #[must_use]
    pub const fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Starts a new step's queue.
    pub(crate) fn clear(&mut self) {
        self.events[..self.len].fill(None);
        self.len = 0;
    }
}
