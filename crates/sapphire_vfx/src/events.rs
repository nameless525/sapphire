//! Events and sub-emitters (M6 N3): a CPU emitter's particles, as one is born, dies or first meets its ground,
//! start another emitter of the same effect where it is, carrying a share of its velocity. The emitter an event
//! names is a sub-emitter: it fires a burst at each event and never on its own.
//!
//! The set is closed, and a library refuses by field what's outside it ([`effect_problem`]): at most
//! [`MAX_EVENTS`] events an emitter, none on a stateless one; a collision event only where the emitter has a
//! ground; an event names a CPU emitter of its own effect, the only one of that name, whose spawn is a burst with
//! no delay; no event leads back to the emitter it started from; and a chain of events is at most
//! [`MAX_EVENT_DEPTH`] long.
//!
//! **Each event draws from a stream of its own** ([`event_stream`]): one keyed by the effect's stream, the
//! event's place and the particle that fired it, which each particle carries from its birth as a plain key, not a
//! random number ([`root_key`], [`child_key`]). So adding an event, and the sub-emitter it names, moves no other
//! particle while both caps have room and each is appended ([`ParticleEvent`] says how far that holds): the
//! effect's own stream draws for its other emitters as it did, and whichever order a step meets its particles in,
//! each firing draws the same numbers.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use glam::Vec3;
use serde::{Deserialize, Serialize};

use crate::document::{Effect, MAX_EMITTERS, Problem, Spawn, require, share_ok};
use crate::rng::{Rng, mix};

/// The most events an emitter has.
pub const MAX_EVENTS: usize = 3;

/// The longest chain of events, from an emitter's particle to a sub-emitter's and on: 2 lets a spark burst into
/// embers that each leave a puff, and no further.
pub const MAX_EVENT_DEPTH: usize = 2;

/// What fires an event.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
pub enum EventTrigger {
    /// A particle is born: where it starts, at the velocity it starts with.
    Birth,
    /// A particle ends, however it does: its life over, inside an attractor's kill radius or on a ground that
    /// ends it. Where it was last, at its velocity then. One cut short by a clear, a reload or a budget's change
    /// fires nothing.
    Death,
    /// A particle meets its ground for the first time ([`Ground`](crate::Ground)), and never again as it bounces or
    /// rests: a splash where a drop lands. On the ground, at the velocity it leaves with, so one the ground ends
    /// keeps only its speed along it.
    Collide,
}

impl EventTrigger {
    /// Its bit in a particle's [`Fires`].
    const fn bit(self) -> u8 {
        match self {
            Self::Birth => 1,
            Self::Death => 2,
            Self::Collide => 4,
        }
    }
}

/// An event of an emitter's particles: when one fires, `emitter` fires its burst where the particle is.
///
/// Adding one moves no other particle under three conditions. The event is appended to its emitter's `events`,
/// and its sub-emitter after the emitters that fire. The world's particle pool has room. So does its step's cap of
/// events ([`VfxBudget::max_events`](crate::VfxBudget::max_events)).
///
/// Past those, the new event changes what else plays:
/// - **A full pool:** a birth that no longer fits is dropped. A dropped birth takes no random numbers, so its
///   effect's later draws shift, and an effect spawned later can lose births.
/// - **A full cap:** the cap is the world's, first met first kept, so the new event can push another effect's
///   events past it.
/// - **An event inserted before another:** an event's stream is keyed by its place in `events`, so the later
///   events' bursts move.
/// - **An emitter inserted before one that fires:** a root particle's key hashes its emitter's place, so that
///   emitter's particles, and every burst they fire, move.
#[derive(Reflect, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParticleEvent {
    pub on: EventTrigger,
    /// The sub-emitter, by its name in the same effect.
    pub emitter: String,
    /// The share of the particle's velocity each of the burst's particles starts with, besides its own: 0 to 1.
    #[serde(default)]
    pub inherit: f32,
}

impl ParticleEvent {
    /// What's wrong with it on its own, its emitter having a ground or not.
    pub(crate) fn problem(&self, has_ground: bool) -> Result<(), Problem> {
        require(
            self.inherit.is_finite() && share_ok(self.inherit),
            "events",
            "an event's `inherit` must be 0 to 1",
        )?;
        require(
            self.on != EventTrigger::Collide || has_ground,
            "events",
            "a collision event needs a ground to meet (`collide`)",
        )
    }
}

/// The triggers an emitter's particles fire events on, as bits: what a particle carries, so a step looks at an
/// emitter's events only where one of them fires.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Fires(u8);

impl Fires {
    pub(crate) fn of(events: &[ParticleEvent]) -> Self {
        Self(events.iter().fold(0, |bits, event| bits | event.on.bit()))
    }

    pub(crate) fn on(self, trigger: EventTrigger) -> bool {
        self.0 & trigger.bit() != 0
    }
}

/// An effect's events resolved: which of its emitters are sub-emitters, and the place of each event's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Links {
    /// A bit an emitter: whether an event names it.
    subs: u8,
    /// Each emitter's events' sub-emitters, by their places.
    targets: [[u8; MAX_EVENTS]; MAX_EMITTERS],
}

impl Links {
    /// `effect`'s, which a library has checked: an event naming no emitter (which a check refuses) links nothing.
    pub(crate) fn of(effect: &Effect) -> Self {
        let mut links = Self::default();
        for (place, emitter) in effect.emitters.iter().enumerate().take(MAX_EMITTERS) {
            for (index, event) in emitter.events.iter().enumerate().take(MAX_EVENTS) {
                if let Some(target) = place_of(effect, &event.emitter) {
                    links.subs |= 1 << target;
                    links.targets[place][index] = target as u8;
                }
            }
        }
        links
    }

    /// Whether the emitter at `place` is a sub-emitter, which fires only at its events.
    pub(crate) fn is_sub(&self, place: usize) -> bool {
        place < MAX_EMITTERS && self.subs & (1 << place) != 0
    }

    /// The place of the sub-emitter that the event at `event` of the emitter at `place` names.
    pub(crate) fn target(&self, place: usize, event: usize) -> usize {
        usize::from(self.targets[place][event])
    }
}

fn place_of(effect: &Effect, name: &str) -> Option<usize> {
    effect.emitters.iter().position(|emitter| emitter.name == name)
}

/// What's wrong with `effect`'s events as a whole, its emitters each valid: the place of the emitter at fault, and
/// its field. An event naming no emitter of the effect, or a stateless one; a sub-emitter that isn't a burst, or
/// waits; events that lead back where they started, or nest past [`MAX_EVENT_DEPTH`].
pub(crate) fn effect_problem(effect: &Effect) -> Result<(), (usize, Problem)> {
    let emitters = &effect.emitters;
    for (place, emitter) in emitters.iter().enumerate() {
        for event in &emitter.events {
            let Some(target) = place_of(effect, &event.emitter) else {
                return Err((place, ("events", "an event names no emitter of its effect")));
            };
            let at = |problem| Err((target, problem));
            if emitters.iter().filter(|other| other.name == event.emitter).count() > 1 {
                return at(("name", "a sub-emitter's name is its own in its effect"));
            }
            if emitters[target].is_stateless() {
                return at(("sim", "a sub-emitter is a CPU emitter"));
            }
            if !matches!(emitters[target].spawn, Spawn::Burst { .. }) {
                return at(("spawn", "a sub-emitter fires a burst at each event"));
            }
            if emitters[target].delay > 0.0 {
                return at(("delay", "a sub-emitter starts at its event, with no delay"));
            }
        }
    }
    // The longest chain of events below each emitter, by a walk that marks the emitters it's inside: one it meets
    // again before it's done leads back to itself.
    let mut depth = [None::<usize>; MAX_EMITTERS];
    let mut inside = [false; MAX_EMITTERS];
    for place in 0..emitters.len().min(MAX_EMITTERS) {
        let below = chain_below(effect, place, &mut depth, &mut inside)
            .map_err(|again| (again, ("events", "its events lead back to it")))?;
        if below > MAX_EVENT_DEPTH {
            return Err((place, ("events", "events chain at most 2 deep")));
        }
    }
    Ok(())
}

/// The longest chain of events below the emitter at `place`, or the place of the emitter the walk met again: one
/// on a loop of events.
fn chain_below(
    effect: &Effect,
    place: usize,
    depth: &mut [Option<usize>; MAX_EMITTERS],
    inside: &mut [bool; MAX_EMITTERS],
) -> Result<usize, usize> {
    if let Some(known) = depth[place] {
        return Ok(known);
    }
    if inside[place] {
        return Err(place);
    }
    inside[place] = true;
    let mut longest = 0;
    for event in &effect.emitters[place].events {
        // The first pass holds that every event names an emitter.
        let Some(target) = place_of(effect, &event.emitter) else { continue };
        longest = longest.max(1 + chain_below(effect, target, depth, inside)?);
    }
    inside[place] = false;
    depth[place] = Some(longest);
    Ok(longest)
}

/// What sets the events' streams apart from an effect's own, and its noises'.
const EVENT_SALT: u64 = 0x6576_656E_7473;

/// Where an effect's events' streams start: forked off its stream as it spawns, which isn't advanced.
pub(crate) fn event_base(stream: &Rng) -> Rng {
    stream.fork(EVENT_SALT)
}

/// The stream of the event at `event` firing for the particle keyed `key`: its own, whatever else fires.
pub(crate) fn event_stream(base: &Rng, key: u64, event: usize) -> Rng {
    base.fork(mix(key ^ (event as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15)))
}

/// The key of the `born`th particle the emitter at `place` fires on its own.
pub(crate) fn root_key(place: usize, born: u32) -> u64 {
    mix(((place as u64) << 32) | u64::from(born))
}

/// The key of the `index`th particle of the burst that the event at `event` fired for the particle keyed `key`.
pub(crate) fn child_key(key: u64, event: usize, index: u32) -> u64 {
    mix(key ^ mix(((event as u64 + 1) << 32) | u64::from(index)))
}

/// An event a step has met and not yet fired: the particle's effect and emitter, which of its events, where and
/// how fast the particle was, and its key.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Pending {
    pub(crate) slot: u16,
    pub(crate) emitter: u8,
    pub(crate) event: u8,
    pub(crate) position: Vec3,
    pub(crate) velocity: Vec3,
    pub(crate) key: u64,
}

/// A step's events, at most the budget's: what doesn't fit is counted and dropped, never kept for the next step.
#[derive(Clone, Debug)]
pub(crate) struct Queue {
    pending: Vec<Pending>,
    cap: usize,
    /// Fired since the simulation began, and dropped past the cap.
    pub(crate) fired: u64,
    pub(crate) dropped: u64,
}

impl Queue {
    pub(crate) fn new(cap: u32) -> Self {
        let cap = cap as usize;
        Self { pending: Vec::with_capacity(cap), cap, fired: 0, dropped: 0 }
    }

    /// Queues one event of the emitter at `place` for each of its events `trigger` fires: allocates nothing.
    pub(crate) fn meet(&mut self, events: &[ParticleEvent], trigger: EventTrigger, mut pending: Pending) {
        for (index, event) in events.iter().enumerate().take(MAX_EVENTS) {
            if event.on != trigger {
                continue;
            }
            if self.pending.len() < self.cap {
                pending.event = index as u8;
                self.pending.push(pending);
            } else {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
    }

    /// The `index`th event this step has queued, if there are that many: a firing may queue more behind it.
    pub(crate) fn get(&self, index: usize) -> Option<Pending> {
        self.pending.get(index).copied()
    }

    /// Ends the step's events, every one of them fired.
    pub(crate) fn finish(&mut self) {
        self.fired = self.fired.saturating_add(self.pending.len() as u64);
        self.pending.clear();
    }

    /// Forgets the step's events unfired: an end of every effect.
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
    }
}
