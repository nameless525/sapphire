//! [`BrainDef`]: how an enemy fights, as numbers a person tunes (M2 G6). It's a section of the same
//! document as the enemy's moves, for the reasons a [`crate::FighterDef`] is: everything a fight
//! depends on has to reach a replay's `tables_hash`, a brain is tuned with the moves it presses, and
//! replacing a set (M2 T2) replaces it too. `sapphire_gameplay`'s `brain` module runs it; the document's
//! `brain:` section is checked by [`MoveSetDocument::compile`](crate::MoveSetDocument::compile), by
//! name, so the numbers below hold whichever way a value was made.
//!
//! The arithmetic here is division and multiplication of authored numbers, which rounds the same on
//! every target, so no reading needs `sapphire_math::det`.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_core::Name;
use sapphire_time::SimDuration;
use serde::{Deserialize, Serialize};

/// Most attacks one brain chooses among: `sapphire_gameplay` remembers when it last used each, in a
/// fixed array, so it never allocates.
pub const MAX_BRAIN_ATTACKS: usize = 8;

/// How long a brain stays at something, drawn when it starts: from `min` to `max` ticks of its own
/// clock, both included.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TickRange {
    pub min: u32,
    pub max: u32,
}

/// How far from its target a brain wants to stand, in metres across the ground: it walks in from past
/// `far`, backs off from inside `near`, and stays between them.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spacing {
    pub near: f32,
    pub far: f32,
}

/// How a brain moves, as a player's thumb would: how far it tilts the stick. The controller's profile
/// says how fast a tilt goes.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainPace {
    /// Walking in, from past [`Spacing::far`]. Within 0..=1, as are the others.
    pub approach: f32,
    /// Backing off, from inside [`Spacing::near`].
    pub retreat: f32,
    /// Circling, between the two.
    pub circle: f32,
    /// How often it circles rather than stands, of the times it starts keeping its distance. Within
    /// 0..=1.
    pub circle_share: f32,
    /// How fast it turns to keep facing its target, in degrees a second.
    pub turn_rate_degrees: f32,
}

/// How a brain behaves on a patrol, in place of looking for a foe however far away it stands. A brain
/// with this and no patrol fights as one without it.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrainWatch {
    /// How near a foe has to come to be noticed, in metres across the ground, with a clear line to it.
    pub sight: f32,
    /// How far off a foe it has noticed can get before it gives up and goes back, past `sight`.
    pub lose: f32,
    /// How far it tilts the stick walking its patrol. Within 0..=1, as a [`BrainPace`]'s are, and not 0.
    pub walk: f32,
}

/// The distances an attack suits: nothing at `near`, the most at `best` and nothing again at `far`, in
/// metres across the ground, in straight lines between.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reach {
    pub near: f32,
    pub best: f32,
    pub far: f32,
}

impl Reach {
    /// How well a target this far away suits it, within 0..=1.
    #[must_use]
    pub fn at(&self, distance: f32) -> f32 {
        // Written so that a NaN scores nothing.
        if !(distance >= self.near && distance <= self.far) {
            0.0
        } else if distance == self.best {
            1.0
        } else if distance < self.best {
            (distance - self.near) / (self.best - self.near)
        } else {
            (self.far - distance) / (self.far - self.best)
        }
    }
}

/// What a brain sees its target doing as it decides. It reads no further than a player reads a screen:
/// what the target is playing, and how far through.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TargetDoing {
    /// Nothing that a move says: standing, walking or running.
    #[default]
    Free,
    /// Winding up or swinging.
    Swinging,
    /// Coming out of an attack: the opening.
    Opening,
    /// Playing one of its own reactions.
    Reacting,
    /// Inside a window where nothing can touch it, as in a dodge.
    Untouchable,
}

/// What an attack's score is multiplied by, for what its target is doing. Each left out is 1.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Against {
    pub free: f32,
    pub swinging: f32,
    pub opening: f32,
    pub reacting: f32,
    pub untouchable: f32,
}

impl Default for Against {
    fn default() -> Self {
        Self { free: 1.0, swinging: 1.0, opening: 1.0, reacting: 1.0, untouchable: 1.0 }
    }
}

impl Against {
    /// The multiplier for what the target is doing.
    #[must_use]
    pub const fn of(&self, doing: TargetDoing) -> f32 {
        match doing {
            TargetDoing::Free => self.free,
            TargetDoing::Swinging => self.swinging,
            TargetDoing::Opening => self.opening,
            TargetDoing::Reacting => self.reacting,
            TargetDoing::Untouchable => self.untouchable,
        }
    }
}

/// How an attack's score falls after it's used, so a brain doesn't repeat itself: it's multiplied by
/// `floor` straight after, climbing in a straight line back to 1 over `ticks` of the brain's clock.
/// Left out, ticks 0: no memory at all.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recent {
    pub ticks: u32,
    pub floor: f32,
}

impl Recent {
    /// The multiplier this long after the attack was last used, or 1 for one never used.
    #[must_use]
    pub fn at(&self, since: Option<SimDuration>) -> f32 {
        let Some(since) = since else { return 1.0 };
        let span = SimDuration::from_ticks(u64::from(self.ticks)).as_subticks();
        if since.as_subticks() >= span {
            return 1.0;
        }
        self.floor + (1.0 - self.floor) * (since.as_subticks() as f32 / span as f32)
    }
}

/// One attack a brain can choose, by its intent. See [`Self::score`].
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BrainAttack {
    /// Its place in the set's intents.
    intent: u32,
    pub weight: f32,
    pub reach: Reach,
    pub against: Against,
    pub recent: Recent,
}

impl BrainAttack {
    pub(crate) const fn new(
        intent: u32,
        weight: f32,
        reach: Reach,
        against: Against,
        recent: Recent,
    ) -> Self {
        Self { intent, weight, reach, against, recent }
    }

    /// The intent it presses, as its place in [`crate::MoveSet::intents`].
    #[must_use]
    pub const fn intent(&self) -> u32 {
        self.intent
    }

    /// How much a brain wants this attack now: its weight, by how well the distance suits it, by what
    /// the target is doing, by how lately it was used. A brain draws among attacks in proportion to
    /// this, and zero is never.
    #[must_use]
    pub fn score(&self, distance: f32, doing: TargetDoing, since: Option<SimDuration>) -> f32 {
        self.weight * self.reach.at(distance) * self.against.of(doing) * self.recent.at(since)
    }

    /// The same factors in a range wide enough for any validated finite weights and multipliers.
    /// Use this when single-precision scores or their total overflow or underflow.
    #[must_use]
    pub fn score_wide(&self, distance: f32, doing: TargetDoing, since: Option<SimDuration>) -> f64 {
        f64::from(self.weight)
            * f64::from(self.reach.at(distance))
            * f64::from(self.against.of(doing))
            * f64::from(self.recent.at(since))
    }
}

/// When a brain gets up: it presses `intent` once, while `during` plays, at a tick drawn between `from`
/// and `to`, `to` not included. The document's check holds that window to one of `during`'s cancels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BrainRise {
    /// Its place in the set's intents.
    intent: u32,
    during: u64,
    pub from: u32,
    pub to: u32,
}

impl BrainRise {
    pub(crate) const fn new(intent: u32, during: Name, from: u32, to: u32) -> Self {
        Self { intent, during: during.as_u64(), from, to }
    }

    /// The intent it presses, as its place in [`crate::MoveSet::intents`].
    #[must_use]
    pub const fn intent(&self) -> u32 {
        self.intent
    }

    /// The move it gets up from.
    #[must_use]
    pub const fn during(&self) -> Name {
        Name::from_u64(self.during)
    }
}

/// How one kind of enemy fights.
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BrainDef {
    pub spacing: Spacing,
    /// How far it likes to stay from others with brains, in metres across the ground. Nothing stops it
    /// coming nearer, but it's pushed away the nearer it comes. 0 keeps no distance at all.
    pub apart: f32,
    pub pace: BrainPace,
    /// How it behaves on a patrol, if it can have one.
    pub watch: Option<BrainWatch>,
    /// How long it keeps its distance before it looks for an attack.
    pub patience: TickRange,
    /// How long it stands after an attack ends: a player's opening.
    pub rest: TickRange,
    /// How long it stands after a reaction ends.
    pub recover: TickRange,
    /// At most [`MAX_BRAIN_ATTACKS`], at least one.
    pub attacks: Vec<BrainAttack>,
    pub rise: Option<BrainRise>,
}
