//! Intents: what an actor wants to do, held for a few ticks until a window can take it (spec §5.22,
//! step 1). A player's buttons fill the ring through [`press_intents`], and an enemy's brain fills
//! the same ring through [`IntentRing::press`] and [`IntentRing::set_held`], so nothing past this
//! file knows which it was.
//!
//! **Time here is the owner's clock.** A press lasts its buffer on the actor's own time, so hitstop
//! can't eat one: pressing through a freeze is exactly when a player buffers. A press is stamped
//! with the reading of the step it arrived on, and remembers where that step began, so a window a
//! big step passed over whole can still count as open when it came.

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_moves::{Button, IntentDef};
use sapphire_time::{LocalClock, SimTime};

use crate::actions::{Buttons, Controls};
use crate::actors::ActorTable;
use crate::moves::{MoveLibrary, MoveRunner, MoveStats};

/// Intents an actor can hold at once: one a tick for the deepest buffer (15 ticks), and one more.
/// A full ring gives up its oldest ([`crate::SimStats::intents_overwritten`]).
pub const INTENT_RING_LEN: usize = 16;

/// One press of an intent, waiting for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Intent {
    pub name: Name,
    /// Which fires when two could at the same instant: the higher.
    pub priority: u8,
    /// The reading of the owner's clock on the step it arrived.
    pub pressed_at: SimTime,
    /// Where that step began on the owner's clock: all of it is when the press could have come.
    pub step_start: SimTime,
    /// The first time on the owner's clock it can no longer fire.
    pub expires_at: SimTime,
}

impl Intent {
    const NONE: Self = Self {
        name: Name::from_u64(0),
        priority: 0,
        pressed_at: SimTime::ZERO,
        step_start: SimTime::ZERO,
        expires_at: SimTime::ZERO,
    };
}

/// An actor's buffered intents, oldest first, and which holds are down.
///
/// Fixed size and plain data: the buffer is what a replay's checksum sees, and what T1's view draws.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct IntentRing {
    /// The live ones first, in the order pressed; those past `len` are cleared, so rings compare
    /// by what they hold.
    entries: [Intent; INTENT_RING_LEN],
    len: u8,
    held: u16,
}

impl Default for IntentRing {
    fn default() -> Self {
        Self { entries: [Intent::NONE; INTENT_RING_LEN], len: 0, held: 0 }
    }
}

impl IntentRing {
    /// Presses an intent on `clock`'s current reading: the one way a press gets in, for buttons and
    /// brains alike. A brain calls this from [`crate::CombatSet::Intents`], after the buttons have
    /// gone in.
    /// Returns whether a full ring gave up its oldest press for it.
    pub fn press(&mut self, intent: &IntentDef, clock: &LocalClock) -> bool {
        let now = clock.now();
        let entry = Intent {
            name: intent.name(),
            priority: intent.priority,
            pressed_at: now,
            step_start: SimTime::from_subticks(
                now.as_subticks().saturating_sub(clock.last_delta().as_subticks()),
            ),
            expires_at: now + intent.buffer(),
        };
        let full = usize::from(self.len) == INTENT_RING_LEN;
        if full {
            self.remove(0);
        }
        self.entries[usize::from(self.len)] = entry;
        self.len += 1;
        full
    }

    /// Which holds are down, a bit for each intent in the set's order
    /// ([`sapphire_moves::MoveSet::intent`]). A player's buttons set this each step; a brain sets it
    /// after that.
    pub fn set_held(&mut self, held: u16) {
        self.held = held;
    }

    /// Which holds are down.
    #[must_use]
    pub const fn held(&self) -> u16 {
        self.held
    }

    /// The buffered presses, oldest first.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &Intent> {
        self.entries[..usize::from(self.len)].iter()
    }

    /// How many presses are buffered.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Whether no press is buffered.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Takes a press out, keeping the rest in order.
    pub(crate) fn remove(&mut self, slot: usize) {
        let len = usize::from(self.len);
        if slot >= len {
            return;
        }
        self.entries[slot..len].rotate_left(1);
        self.entries[len - 1] = Intent::NONE;
        self.len -= 1;
    }

    /// Drops presses that can't fire any more at or after `now`, and returns how many.
    pub(crate) fn expire(&mut self, now: SimTime) -> u64 {
        let mut kept = 0;
        for slot in 0..usize::from(self.len) {
            if self.entries[slot].expires_at > now {
                self.entries[kept] = self.entries[slot];
                kept += 1;
            }
        }
        let expired = usize::from(self.len) - kept;
        for slot in kept..usize::from(self.len) {
            self.entries[slot] = Intent::NONE;
        }
        self.len = kept as u8;
        expired as u64
    }

    /// The buffered presses, to change, for tests of what's watching them.
    #[cfg(test)]
    pub(crate) fn entries_mut(&mut self) -> &mut [Intent] {
        &mut self.entries[..usize::from(self.len)]
    }

    /// Forgets everything: a new actor's ring.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }
}

impl From<Button> for Buttons {
    fn from(button: Button) -> Self {
        Self::from_bits(button.bit()).expect("every button a document can name is a real one")
    }
}

/// [`crate::CombatSet::Intents`]: turns each actor's buttons into intents through its set's table, on
/// their press edge, and records which are held.
pub(crate) fn press_intents(
    table: Res<'_, ActorTable>,
    library: Res<'_, MoveLibrary>,
    mut stats: ResMut<'_, MoveStats>,
    mut actors: Query<'_, '_, (&Controls, &EntityClock, &MoveRunner, &mut IntentRing)>,
) {
    for (_, entity) in table.iter() {
        let Ok((controls, clock, runner, mut ring)) = actors.get_mut(entity) else { continue };
        let Some(set) = runner.set().and_then(|set| library.current(set)) else { continue };
        let pressed = controls.pressed();
        let mut held = 0;
        for (bit, intent) in set.intents().iter().enumerate() {
            let Some(button) = intent.button else { continue };
            let button = Buttons::from(button);
            if controls.current.buttons.contains(button) {
                held |= 1 << bit;
            }
            if pressed.contains(button) {
                stats.intents_overwritten += u64::from(ring.press(intent, &clock.0));
                stats.intents_pressed += 1;
            }
        }
        ring.set_held(held);
    }
}
