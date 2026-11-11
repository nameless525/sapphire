//! The player's timing assist (spec §6.8, M4 A6): a setting that widens the perfect-dodge and counter windows
//! of the actors a player plays, by a fixed number of ticks a step.
//!
//! **It widens in the simulation,** not in input, so a step reads it as it reads a move's own windows and a
//! replay holds. A perfect-dodge window stays open [`AssistWidths::dodge`] ticks longer at its end, never past
//! the i-frames running on from it ([`sapphire_moves::Move::widened_dodge`]), so a hit and a perfect dodge
//! still can't land on one actor in one step; a fired dodge's counter window stays open
//! [`AssistWidths::counter`] ticks longer. Nothing opens earlier. An actor a brain plays is never assisted, so
//! a foe that dodges doesn't get better at it when the player asks for help.
//!
//! **The numbers are a document** (`default_timing_assist.ron`, [`TimingAssistTuning`]), compiled in because a
//! replay needs no assets. The step is the run's ([`crate::Simulation::set_timing_assist`]): a player changes
//! it between steps, a recording keeps that as a command, and the tables' hash takes the step and its numbers
//! only while it's on, so a run with it off hashes and records as one from before there was a setting.

use std::sync::OnceLock;

use bevy_reflect::{Reflect, TypePath};
use sapphire_ecs::prelude::*;
use sapphire_moves::MAX_MOVE_TICKS;
use sapphire_reflect::Versioned;
use sapphire_serialize::DocumentError;
use sapphire_time::SimDuration;
use serde::{Deserialize, Serialize};

use crate::brain::Brain;
use crate::checksum::StateHasher;

/// The shipped numbers (see [`TimingAssistTuning::shipped`]).
const SHIPPED: &str = include_str!("../default_timing_assist.ron");

/// How far the timing assist widens: a player's setting, which a step reads, so the run records it. Off is
/// what every run before the setting ran on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Reflect, Resource)]
#[type_path = "sapphire::gameplay"]
pub enum TimingAssist {
    #[default]
    Off,
    /// [`TimingAssistTuning::wide`].
    Wide,
    /// [`TimingAssistTuning::wider`].
    Wider,
}

impl TimingAssist {
    /// Every step, off first: the order a setting steps through them.
    pub const ALL: [Self; 3] = [Self::Off, Self::Wide, Self::Wider];

    /// Its number in a replay's command.
    #[must_use]
    pub const fn to_raw(self) -> u32 {
        self as u32
    }

    /// The step `raw` numbers, if it numbers one.
    #[must_use]
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Off),
            1 => Some(Self::Wide),
            2 => Some(Self::Wider),
            _ => None,
        }
    }
}

/// What one step adds, in ticks of the actor's own clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssistWidths {
    /// To a perfect-dodge window's end, as far as the i-frames running on from it allow.
    pub dodge: u32,
    /// To a fired perfect dodge's counter window.
    pub counter: u32,
}

impl AssistWidths {
    /// Nothing widened: the assist off, or an actor a brain plays.
    pub const NONE: Self = Self { dodge: 0, counter: 0 };

    /// What it adds to a counter window, on the dodger's clock.
    #[must_use]
    pub const fn counter_duration(self) -> SimDuration {
        SimDuration::from_ticks(self.counter as u64)
    }
}

/// How far each step of the timing assist widens (a `TimingAssistTuning` document, schema 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TypePath, Resource)]
#[type_path = "sapphire::gameplay"]
#[serde(deny_unknown_fields)]
pub struct TimingAssistTuning {
    pub wide: AssistWidths,
    /// At least [`Self::wide`]'s, each.
    pub wider: AssistWidths,
}

impl Versioned for TimingAssistTuning {
    /// 1: M4 A6.
    const VERSION: u32 = 1;
}

/// A timing assist document that can't be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TimingAssistError {
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[error("{step}: {reason}")]
    Step { step: &'static str, reason: String },
}

impl TimingAssistTuning {
    /// Sapphire's numbers (`default_timing_assist.ron` beside this crate's manifest), compiled in.
    #[must_use]
    pub fn shipped() -> Self {
        static PARSED: OnceLock<TimingAssistTuning> = OnceLock::new();
        *PARSED.get_or_init(|| Self::from_ron(SHIPPED).expect("this module's tests read the shipped numbers"))
    }

    /// Reads a timing assist document and checks it: each step widens something, by no more than a move can
    /// last, and the wider step is wider in both.
    pub fn from_ron(text: &str) -> Result<Self, TimingAssistError> {
        let tuning: Self = sapphire_serialize::from_ron(text)?;
        for (step, widths) in [("wide", tuning.wide), ("wider", tuning.wider)] {
            let refuse = |reason: String| Err(TimingAssistError::Step { step, reason });
            if widths == AssistWidths::NONE {
                return refuse("widens nothing, which is what off is".to_owned());
            }
            if widths.dodge > MAX_MOVE_TICKS || widths.counter > MAX_MOVE_TICKS {
                return refuse(format!("widens by more than a move's {MAX_MOVE_TICKS} ticks"));
            }
        }
        if tuning.wider.dodge < tuning.wide.dodge || tuning.wider.counter < tuning.wide.counter {
            return Err(TimingAssistError::Step {
                step: "wider",
                reason: "widens less than wide does".to_owned(),
            });
        }
        Ok(tuning)
    }

    /// What `step` adds to each window.
    #[must_use]
    pub const fn widths(&self, step: TimingAssist) -> AssistWidths {
        match step {
            TimingAssist::Off => AssistWidths::NONE,
            TimingAssist::Wide => self.wide,
            TimingAssist::Wider => self.wider,
        }
    }

    /// What `step` adds to the windows of an actor with `brain`: nothing when a brain plays it.
    #[must_use]
    pub(crate) fn widths_for(&self, step: TimingAssist, brain: Option<&Brain>) -> AssistWidths {
        if brain.is_some_and(|brain| brain.on) { AssistWidths::NONE } else { self.widths(step) }
    }

    /// What [`crate::Simulation::tables_hash`] takes of them at `step`: the step and the numbers it reads, by
    /// their bits. Off, nothing at all, so the hash is what it was before there was a setting.
    pub(crate) fn write_tables(&self, step: TimingAssist, hasher: &mut StateHasher) {
        if step == TimingAssist::Off {
            return;
        }
        let widths = self.widths(step);
        hasher.bytes(b"timing-assist");
        hasher.u32(step.to_raw());
        hasher.u32(widths.dodge);
        hasher.u32(widths.counter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_document_widens_more_at_each_step() {
        let shipped = TimingAssistTuning::shipped();
        assert_eq!(shipped.widths(TimingAssist::Off), AssistWidths::NONE);
        let (wide, wider) = (shipped.widths(TimingAssist::Wide), shipped.widths(TimingAssist::Wider));
        assert!(wide.dodge > 0 && wide.counter > 0, "{wide:?}");
        assert!(wider.dodge > wide.dodge && wider.counter > wide.counter, "{wider:?}");
        for step in TimingAssist::ALL {
            assert_eq!(TimingAssist::from_raw(step.to_raw()), Some(step));
        }
        assert_eq!(TimingAssist::from_raw(3), None);
    }

    /// A typo or a step that widens nothing, too much or less than the one before is refused by name.
    #[test]
    fn a_document_that_cant_be_used_is_refused_by_name() {
        let unknown = SHIPPED.replacen("counter:", "countr:", 1);
        let refused = TimingAssistTuning::from_ron(&unknown).expect_err("a typo");
        assert!(refused.to_string().contains("countr"), "{refused}");
        for (from, to, says) in [
            ("wide: (dodge: 3, counter: 15)", "wide: (dodge: 0, counter: 0)", "wide: widens nothing"),
            ("wider: (dodge: 6, counter: 30)", "wider: (dodge: 6, counter: 9000)", "wider: widens by more"),
            ("wider: (dodge: 6, counter: 30)", "wider: (dodge: 2, counter: 30)", "wider: widens less"),
        ] {
            assert!(SHIPPED.contains(from), "{from}");
            let refused = TimingAssistTuning::from_ron(&SHIPPED.replacen(from, to, 1)).expect_err(to);
            assert!(refused.to_string().contains(says), "{to}: {refused}");
        }
    }

    /// Off writes nothing to the tables' hash; each step writes itself and the numbers it reads, so another
    /// step or other numbers can't pass for it.
    #[test]
    fn the_tables_take_the_step_and_its_numbers_only_while_its_on() {
        let digest = |tuning: &TimingAssistTuning, step| {
            let mut hasher = StateHasher::new();
            tuning.write_tables(step, &mut hasher);
            hasher.finish()
        };
        let shipped = TimingAssistTuning::shipped();
        let mut other = shipped;
        other.wider.counter += 1;
        other.wide.dodge += 1;
        assert_eq!(digest(&shipped, TimingAssist::Off), StateHasher::new().finish());
        assert_eq!(digest(&other, TimingAssist::Off), digest(&shipped, TimingAssist::Off));
        assert_ne!(digest(&shipped, TimingAssist::Wide), digest(&shipped, TimingAssist::Off));
        assert_ne!(digest(&shipped, TimingAssist::Wider), digest(&shipped, TimingAssist::Wide));
        assert_ne!(digest(&other, TimingAssist::Wide), digest(&shipped, TimingAssist::Wide));
        assert_ne!(digest(&other, TimingAssist::Wider), digest(&shipped, TimingAssist::Wider));
    }

    #[test]
    fn a_brain_is_never_assisted() {
        let shipped = TimingAssistTuning::shipped();
        let playing = Brain { on: true, ..Brain::default() };
        let idle = Brain::default();
        assert_eq!(shipped.widths_for(TimingAssist::Wider, Some(&playing)), AssistWidths::NONE);
        assert_eq!(shipped.widths_for(TimingAssist::Wider, Some(&idle)), shipped.wider, "a brain that's off");
        assert_eq!(shipped.widths_for(TimingAssist::Wide, None), shipped.wide);
    }
}
