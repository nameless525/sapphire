//! The cue queue: what a step tells presentation happened (spec §5.22, step 8, rule R3).
//!
//! Moves and hits publish here and nothing in the simulation reads it back. M2 E3 drains it into
//! effects, sound, shake, rumble and P3's post presets, which is how the simulation stays headless. It
//! holds one step's events: read it after each [`crate::Simulation::step`], as an app running two
//! steps a frame does twice.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;
use sapphire_moves::Weight;
use sapphire_time::{SimDuration, SimTime};

use crate::actors::ActorId;
use crate::resolve::HitOutcome;

/// Most cues one step can publish: room for every hit a step can land with a second cue each, and a
/// full pool's moves besides. A step's are a handful per actor, so past it they're counted and dropped
/// ([`crate::SimStats::cues_dropped`]), since losing a sound is better than failing a fight. 64 lost a
/// third of `step_bench`'s full pool's.
pub const CUE_QUEUE_LEN: usize = 192;
const _: () = assert!(CUE_QUEUE_LEN >= 2 * crate::hits::MAX_HIT_EVENTS + 64);

/// Published after a hit's own cue, when it took the last of its target's poise.
pub const POISE_BROKEN: Name = Name::new("poise_broken");

/// Published after a hit's own cue, when it took the last of its target's health.
pub const DEFEATED: Name = Name::new("defeated");

/// Published when a perfect dodge fires, with the dodger for its actor. It's what E3 hangs a perfect
/// dodge's flash and sound on, in the step the world starts to slow.
pub const PERFECT_DODGE: Name = Name::new("perfect_dodge");

/// What a hit's cues carry: everything it landed with, for what presentation hangs on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitCue {
    /// Who it landed on. The cue's [`CueEvent::actor`] is who landed it.
    pub target: ActorId,
    /// Which of the target's hurtboxes.
    pub hurtbox: u8,
    /// On the hurtbox's surface, in the world.
    pub point: Vec3,
    /// Away from the hurtbox, toward the volume.
    pub normal: Vec3,
    pub weight: Weight,
    pub damage: u32,
    pub poise: u32,
    /// How long it freezes its owners, for impact frames as long as the freeze (spec §5.21's P3 note).
    pub hitstop: u16,
    pub outcome: HitOutcome,
}

/// A presentation event, with exactly when it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CueEvent {
    /// Whose it is: the mover's, a hit's attacker or a perfect dodge's dodger.
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
    /// What a hit landed with, on the cues a hit publishes.
    pub hit: Option<HitCue>,
}

/// One step's cues: its moves' in the order they fired, each actor's in time order, then its perfect
/// dodges' by dodger, then its hits' in the order they applied. Nothing in the simulation reads it back.
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
