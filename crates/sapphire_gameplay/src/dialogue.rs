//! What a dialogue does to the simulation (spec §5.23, M3 Q2): its effects, which set flags, and nothing
//! else of it. The graph, its lines and its conditions are presentation's (`sapphire_dialogue`'s walk), and
//! a run where someone read every line and chose nothing is a run with no dialogue in it.
//!
//! A dialogue's simulation half is loaded with
//! [`Simulation::add_dialogue`](crate::Simulation::add_dialogue), between steps, where it allocates and
//! goes in [`tables_hash`](crate::Simulation::tables_hash). An effect is applied with
//! [`Simulation::apply_dialogue_effect`](crate::Simulation::apply_dialogue_effect), between steps, and a
//! replay records it as a [`ReplayCommand::Dialogue`](crate::replay::ReplayCommand): something a player
//! did, which a run's own state can't tell, so it's input. Its writes follow from the dialogue's digest.

use sapphire_dialogue::{DialogueError, DialogueSimulation, EffectId};
use sapphire_ecs::prelude::*;

use crate::checksum::StateHasher;
use crate::flags::Flags;
use sapphire_core::Name;

/// A loaded dialogue's simulation half, by the order it was added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DialogueId(u16);

impl DialogueId {
    /// The ID a run's `add_dialogue`s gave it, from a replay's file or a script.
    #[must_use]
    pub const fn from_raw(raw: u16) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// One effect of one loaded dialogue: what a replay records when a choice or a line sets flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DialogueEffect {
    pub dialogue: DialogueId,
    pub effect: EffectId,
}

impl DialogueEffect {
    /// Both, in a replay command's one number: the dialogue in the high half, the effect in the low.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        ((self.dialogue.0 as u32) << 16) | self.effect.0 as u32
    }

    /// [`Self::to_raw`]'s inverse.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self { dialogue: DialogueId((raw >> 16) as u16), effect: EffectId(raw as u16) }
    }
}

/// Why an effect wasn't applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DialogueEffectError {
    #[error("no dialogue {} is loaded", .0.raw())]
    Unknown(DialogueId),
    #[error("dialogue {} has no effect {}", .0.dialogue.raw(), .0.effect.0)]
    NoSuchEffect(DialogueEffect),
}

/// The loaded dialogues' simulation halves. Nothing here changes during a step.
#[derive(Resource, Debug, Default)]
pub struct DialogueLibrary {
    dialogues: Vec<(DialogueSimulation, u64)>,
    applied: u64,
}

impl DialogueLibrary {
    pub(crate) fn add(&mut self, dialogue: DialogueSimulation) -> Result<DialogueId, DialogueError> {
        dialogue.validate()?;
        let Ok(raw) = u16::try_from(self.dialogues.len()) else {
            return Err(DialogueError::Invalid {
                field: "dialogue".to_owned(),
                problem: "a simulation holds at most 65,536 dialogues".to_owned(),
            });
        };
        let digest = dialogue.digest();
        self.dialogues.push((dialogue, digest));
        Ok(DialogueId(raw))
    }

    /// Applies an effect's writes, in order.
    pub(crate) fn apply(
        &mut self,
        effect: DialogueEffect,
        flags: &mut Flags,
    ) -> Result<(), DialogueEffectError> {
        let (dialogue, _) = self
            .dialogues
            .get(usize::from(effect.dialogue.0))
            .ok_or(DialogueEffectError::Unknown(effect.dialogue))?;
        let writes = &dialogue
            .effects
            .get(usize::from(effect.effect.0))
            .ok_or(DialogueEffectError::NoSuchEffect(effect))?
            .writes;
        for write in writes {
            flags.set(Name::from_u64(write.flag), write.value);
        }
        self.applied += 1;
        Ok(())
    }

    /// The digest of dialogue `id`'s simulation half, if it's loaded.
    #[must_use]
    pub fn digest(&self, id: DialogueId) -> Option<u64> {
        self.dialogues.get(usize::from(id.0)).map(|&(_, digest)| digest)
    }

    /// Effects applied so far (rule R9).
    #[must_use]
    pub const fn applied(&self) -> u64 {
        self.applied
    }

    /// Goes in `tables_hash`, and only with something to say, so a run on no dialogues hashes as it did.
    pub(crate) fn write_digests(&self, hasher: &mut StateHasher) {
        if self.dialogues.is_empty() {
            return;
        }
        hasher.u32(self.dialogues.len() as u32);
        for &(_, digest) in &self.dialogues {
            hasher.u64(digest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_effect_goes_in_one_number_and_comes_back() {
        for (dialogue, effect) in [(0, 0), (1, 2), (u16::MAX, u16::MAX), (300, 7)] {
            let effect = DialogueEffect { dialogue: DialogueId(dialogue), effect: EffectId(effect) };
            assert_eq!(DialogueEffect::from_raw(effect.to_raw()), effect);
        }
    }
}
