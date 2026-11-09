//! Walking a dialogue: where a conversation has got to, and what a panel shows of it.
//!
//! A [`DialogueWalk`] is a place in a [`Dialogue`]'s graph and nothing more, so it allocates nothing and
//! holds no borrow. It goes forward only when told, by [`DialogueWalk::next`] or [`DialogueWalk::choose`],
//! which match a panel's two commands, and [`DialogueWalk::view`] says what to show: who speaks, the line,
//! its voice cue, and the choices whose `when` holds, in the document's order.
//!
//! The walk doesn't know how a condition is asked or an effect applied: whoever walks it says, through a
//! [`DialogueWorld`]. A game asks conditions of the script VM between steps (`sapphire_script`) and applies
//! effects as recorded commands (`sapphire_gameplay`), in the order the walk meets them, so a branch after a
//! choice reads the flags that choice set.

use sapphire_core::Name;

use crate::cooked::{CookedNode, Dialogue};
use crate::document::MAX_CHOICES;

/// A condition of a dialogue: the function at this place in its chunk's `conditions`, from 0.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ConditionId(pub u16);

/// An effect of a dialogue: [`DialogueSimulation::effects`](crate::DialogueSimulation)'s place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct EffectId(pub u16);

/// How a walk asks and acts.
pub trait DialogueWorld {
    /// Whether a condition holds now.
    fn ask(&mut self, condition: ConditionId) -> bool;
    /// Applies an effect, before the walk asks anything more.
    fn apply(&mut self, effect: EffectId);
}

/// What a panel shows: see the [module docs](self). Keys, for `sapphire_loc`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogueView<'a> {
    pub speaker: &'a str,
    pub line: &'a str,
    pub vo: Option<&'a str>,
    /// The choices shown, from the first slot, in the document's order: [`DialogueWalk::choose`] takes a
    /// slot.
    pub choices: [Option<ChoiceView<'a>>; MAX_CHOICES],
    /// Whether this line goes on by "next" rather than a choice, which ends the conversation if nothing
    /// follows it.
    pub next: bool,
}

/// One choice a panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChoiceView<'a> {
    pub line: &'a str,
}

/// Where a conversation has got to: see the [module docs](self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DialogueWalk {
    /// The node whose line is shown, or `None` once it's over.
    at: Option<u16>,
    /// Each slot's choice, by its place in the node's.
    shown: [Option<u8>; MAX_CHOICES],
}

impl DialogueWalk {
    /// Starts a conversation: runs its branches to the first line and applies what showing that sets.
    pub fn begin(dialogue: &Dialogue, world: &mut impl DialogueWorld) -> Self {
        let mut walk = Self { at: None, shown: [None; MAX_CHOICES] };
        walk.enter(dialogue, Some(dialogue.presentation.start), world);
        walk
    }

    /// Goes on from a line with no choices. Says whether it went anywhere: not from a line with choices,
    /// or once the conversation's over.
    pub fn next(&mut self, dialogue: &Dialogue, world: &mut impl DialogueWorld) -> bool {
        let Some(node) = self.node(dialogue) else { return false };
        if !node.choices.is_empty() {
            return false;
        }
        let next = node.next;
        self.enter(dialogue, next, world);
        true
    }

    /// Takes the choice shown in `slot`, applies what it sets, and goes where it leads. Says whether there
    /// was such a choice.
    pub fn choose(&mut self, dialogue: &Dialogue, slot: u8, world: &mut impl DialogueWorld) -> bool {
        let Some(node) = self.node(dialogue) else { return false };
        let Some(Some(place)) = self.shown.get(usize::from(slot)).copied() else { return false };
        let choice = &node.choices[usize::from(place)];
        let (effect, to) = (choice.effect, choice.to);
        if let Some(effect) = effect {
            world.apply(EffectId(effect));
        }
        self.enter(dialogue, to, world);
        true
    }

    /// Whether the conversation has ended.
    #[must_use]
    pub const fn is_over(&self) -> bool {
        self.at.is_none()
    }

    /// The name of the node whose line is shown, as a hash: what a save keeps to come back to it.
    #[must_use]
    pub fn at(&self, dialogue: &Dialogue) -> Option<Name> {
        self.node(dialogue).map(|node| Name::from_u64(node.name))
    }

    /// What to show, or `None` once it's over.
    #[must_use]
    pub fn view<'a>(&self, dialogue: &'a Dialogue) -> Option<DialogueView<'a>> {
        let node = self.node(dialogue)?;
        let say = node.say.as_ref().expect("a walk stops only at a line");
        let mut choices = [None; MAX_CHOICES];
        for (slot, place) in choices.iter_mut().zip(self.shown) {
            *slot = place.map(|place| ChoiceView { line: &node.choices[usize::from(place)].line });
        }
        Some(DialogueView {
            speaker: &say.speaker,
            line: &say.line,
            vo: say.vo.as_deref(),
            choices,
            next: node.choices.is_empty(),
        })
    }

    fn node<'a>(&self, dialogue: &'a Dialogue) -> Option<&'a CookedNode> {
        dialogue.presentation.nodes.get(usize::from(self.at?))
    }

    /// Goes to `to`, through any branches, to a line or the end. A validated graph has no loop of
    /// branches, so it stops within a pass over the nodes; the bound only keeps a bad one from spinning.
    fn enter(&mut self, dialogue: &Dialogue, mut to: Option<u16>, world: &mut impl DialogueWorld) {
        let nodes = &dialogue.presentation.nodes;
        self.shown = [None; MAX_CHOICES];
        for _ in 0..=nodes.len() {
            let Some(node) = to.and_then(|at| nodes.get(usize::from(at))) else { break };
            if node.say.is_none() {
                let arm = node.branch.iter().find(|arm| world.ask(ConditionId(arm.when)));
                to = arm.map(|arm| arm.to).or(node.otherwise);
                continue;
            }
            if let Some(effect) = node.effect {
                world.apply(EffectId(effect));
            }
            let mut slot = 0;
            for (place, choice) in node.choices.iter().enumerate() {
                if choice.when.is_none_or(|when| world.ask(ConditionId(when))) {
                    self.shown[slot] = Some(place as u8);
                    slot += 1;
                }
            }
            self.at = to;
            return;
        }
        self.at = None;
    }
}
