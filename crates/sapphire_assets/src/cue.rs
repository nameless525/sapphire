//! Cues (`.cue.ron` documents, cooked to `.scue`; M2 E1, spec §5.15): what a designer writes to say
//! how a sound plays. Gameplay asks for a cue, never a sound.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::Cue",
//!     version: 3,
//!     data: (
//!         // Footsteps: on grass one of two, never the same twice running; on anything else, stone's.
//!         play: Switch((
//!             on: "surface",
//!             cases: [
//!                 (when: "stone", play: Random((of: [
//!                     Sound("4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c"),
//!                     Sound("0e1f2a3b-4c5d-4e6f-8a9b-0c1d2e3f4a5b"),
//!                 ]))),
//!                 (when: "grass", play: Random((
//!                     of: [
//!                         Sound("6a7b8c9d-0e1f-4a2b-8c3d-4e5f6a7b8c9d"),
//!                         Sound("1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
//!                     ],
//!                     avoid_repeat: 1,
//!                 ))),
//!             ],
//!             default: "stone",
//!         )),
//!         volume_db: (-6.0, -3.0),
//!         pitch: (0.95, 1.05),
//!         attenuation: Some((near: 1.0, far: 30.0, curve: Inverse)),
//!         bus: Sfx,
//!         hitstop: Duck(-12.0),
//!         // M6 U3: how it competes for the mixer's voices.
//!         priority: 40,
//!         limit: Some((max: 4, rule: StopOldest)),
//!         group: Some("footsteps"),
//!     ),
//! )
//! ```
//!
//! Only `play` is required. [`Cue::from_document`] reads a document and says what's wrong with it
//! by field, or by the document's own line; the cook refuses a cue whose sounds aren't sounds of
//! the content tree. The cooked form is the same type, so a cue loads as an asset whose dependencies
//! are its sounds ([`Cue::sounds`]).
//!
//! A cue names what is authored. Walking its tree, picking among its ranges, placing the voices and mixing
//! them are `sapphire_audio`'s.
//!
//! **Version 2 (M6 U3)** adds how a cue competes for the mixer's voices, each field optional with what a version 1
//! cue did as its default: a [`priority`](Cue::priority), a [`limit`](Cue::limit) on its own voices at once, a
//! [`group`](Cue::group) of the mix whose limit it shares, and whether it's [`looped`](Cue::looped).
//!
//! **Version 3 (M6 U4a)** makes what a cue plays a tree ([`CueNode`]) in place of version 2's list of sounds:
//! sounds at its leaves, and above them a random pick (by weight, keeping clear of its last few), a sequence, a
//! layer, a switch on a named parameter and a blend along one. A version 1 or 2 document reads as it was
//! written, its `sounds` a random node of equal weights, which picks exactly as the list did.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::DocumentError;
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::{Deserialize, Serialize};

use crate::AssetId;

/// Conventional file extension for cooked cues.
pub const CUE_EXTENSION: &str = "scue";

/// The quietest gain that still counts as sound, in decibels: where a far attenuation ends.
pub const SILENCE_DB: f32 = -60.0;

/// The loudest a cue may ask for, in decibels.
pub(crate) const MAX_VOLUME_DB: f32 = 24.0;

/// The playback rates a cue may ask for: a tenth of the speed to eight times it.
const PITCH_RANGE: std::ops::RangeInclusive<f32> = 0.1..=8.0;

/// A cue's priority when it names none (M6 U3): the middle, so a designer can rank a sound above or below
/// everything left as it was.
pub const DEFAULT_PRIORITY: u8 = 128;

/// The most voices a [`Limit`] may allow: more than the mixer keeps, real and virtual, is no limit.
pub const MAX_LIMIT: u16 = 512;

/// The most sounds one start of a cue may play at once (M6 U4a): what its layers and blends can reach together.
pub const MAX_START_SOUNDS: usize = 8;

/// The most of its last picks a [`RandomNode`] may keep clear of.
pub const MAX_AVOID_REPEAT: u8 = 8;

/// The deepest a cue's tree may go, its root at 1.
pub const MAX_TREE_DEPTH: usize = 8;

/// The most nodes a cue's tree may have, sounds among them.
pub const MAX_TREE_NODES: usize = 1024;

/// A cue. See the [module docs](self).
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct Cue {
    /// What a start plays (M6 U4a): a tree with sounds at its leaves, at least one.
    pub play: CueNode,
    /// The volume to pick from, least to most, in decibels: 0 is the sound as authored. Every sound a start
    /// plays takes the same pick.
    #[serde(default)]
    pub volume_db: [f32; 2],
    /// The playback rate to pick from, least to most: 1 is the sound as authored. It moves pitch
    /// and speed together.
    #[serde(default = "authored_pitch")]
    pub pitch: [f32; 2],
    /// How the cue fades with distance from the listener, and whether it is placed in the world at
    /// all: `None` plays flat, at its volume and centered, wherever it was asked for (interface
    /// sounds).
    #[serde(default)]
    pub attenuation: Option<Attenuation>,
    /// The mixer bus it plays on.
    #[serde(default)]
    pub bus: Bus,
    /// What it does while the actor it is attached to is in hitstop.
    #[serde(default)]
    pub hitstop: HitstopResponse,
    /// How much its voices matter against others when the mixer has more to play than it mixes (M6 U3): higher
    /// keeps a real voice first. [`DEFAULT_PRIORITY`] if it names none.
    #[serde(default = "default_priority")]
    pub priority: u8,
    /// How many of its voices may sound at once, and which goes when one more is asked for. None, and it
    /// competes only by its priority, its group's limit and the mixer's caps.
    #[serde(default)]
    pub limit: Option<Limit>,
    /// The voice group of the mix it counts toward ([`Mix::groups`](crate::mix::Mix::groups)), whose limit its
    /// voices share with every other cue naming it. A group the mix hasn't limits nothing, and is counted as it plays
    /// (`AudioStats::groups_unknown`); `sapphire_pipeline check` refuses one no mix of the tree has.
    #[serde(default)]
    pub group: Option<String>,
    /// Whether it plays round and round until it's ended: on an emitter, whose going ends it. A virtual loop
    /// keeps its place and comes back where it would be.
    #[serde(default)]
    pub looped: bool,
}

/// Every field as a document that leaves it out has it, and a random pick of no sounds yet: a cue to fill in,
/// which [`Cue::check`] refuses until it names one.
impl Default for Cue {
    fn default() -> Self {
        Self {
            play: CueNode::one_of(Vec::new()),
            volume_db: [0.0; 2],
            pitch: authored_pitch(),
            attenuation: None,
            bus: Bus::default(),
            hitstop: HitstopResponse::default(),
            priority: DEFAULT_PRIORITY,
            limit: None,
            group: None,
            looped: false,
        }
    }
}

fn authored_pitch() -> [f32; 2] {
    [1.0, 1.0]
}

fn default_priority() -> u8 {
    DEFAULT_PRIORITY
}

impl Versioned for Cue {
    /// Version 3 (M6 U4a): `play`, a tree, in place of `sounds`, which reads as a random node of equal weights.
    const VERSION: u32 = 3;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode::<v1::Cue>().map(|cue| v2::Cue::from(cue).into()).map_err(VersionError::Decode),
            2 => data.decode::<v2::Cue>().map(Into::into).map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
}

/// Version 2 (M6 U3), as its documents read: a list of sounds, one picked a start, and how the cue competes.
mod v2 {
    use serde::Deserialize;

    use super::{Attenuation, Bus, CueNode, HitstopResponse, Limit, authored_pitch, default_priority};
    use crate::AssetId;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct Cue {
        pub(super) sounds: Vec<AssetId>,
        #[serde(default)]
        pub(super) volume_db: [f32; 2],
        #[serde(default = "authored_pitch")]
        pub(super) pitch: [f32; 2],
        #[serde(default)]
        pub(super) attenuation: Option<Attenuation>,
        #[serde(default)]
        pub(super) bus: Bus,
        #[serde(default)]
        pub(super) hitstop: HitstopResponse,
        #[serde(default = "default_priority")]
        pub(super) priority: u8,
        #[serde(default)]
        pub(super) limit: Option<Limit>,
        #[serde(default)]
        pub(super) group: Option<String>,
        #[serde(default)]
        pub(super) looped: bool,
    }

    impl From<Cue> for super::Cue {
        /// The list as a random node of equal weights, which picks the sound a version 2 cue did with the same
        /// number: the number's remainder by the count.
        fn from(cue: Cue) -> Self {
            let Cue { sounds, volume_db, pitch, attenuation, bus, hitstop, priority, limit, group, looped } =
                cue;
            Self {
                play: CueNode::one_of(sounds),
                volume_db,
                pitch,
                attenuation,
                bus,
                hitstop,
                priority,
                limit,
                group,
                looped,
            }
        }
    }
}

/// Version 1 (M2 E1), as its documents read: what a cue says of its sound, and nothing of how it competes.
mod v1 {
    use serde::Deserialize;

    use super::{Attenuation, Bus, DEFAULT_PRIORITY, HitstopResponse, authored_pitch};
    use crate::AssetId;

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct Cue {
        sounds: Vec<AssetId>,
        #[serde(default)]
        volume_db: [f32; 2],
        #[serde(default = "authored_pitch")]
        pitch: [f32; 2],
        #[serde(default)]
        attenuation: Option<Attenuation>,
        #[serde(default)]
        bus: Bus,
        #[serde(default)]
        hitstop: HitstopResponse,
    }

    impl From<Cue> for super::v2::Cue {
        fn from(Cue { sounds, volume_db, pitch, attenuation, bus, hitstop }: Cue) -> Self {
            Self {
                sounds,
                volume_db,
                pitch,
                attenuation,
                bus,
                hitstop,
                priority: DEFAULT_PRIORITY,
                limit: None,
                group: None,
                looped: false,
            }
        }
    }
}

/// What a cue plays (M6 U4a): a sound, or a node choosing among or combining the nodes under it. A start walks it
/// from the root, and plays the sounds it reaches, [`MAX_START_SOUNDS`] at most.
///
/// However the tree is shaped, a start draws the same count of random numbers (`sapphire_audio`'s rule): the
/// random nodes it reaches take the first, then numbers made from it, so one cue's shape moves no other's picks.
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
// The tree holds itself, so its archive's bounds are spelled out rather than derived from its fields.
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Writer + rkyv::ser::Allocator, __S::Error: rkyv::rancor::Source),
    deserialize_bounds(__D::Error: rkyv::rancor::Source),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext, __C::Error: rkyv::rancor::Source)),
    derive(Debug)
)]
#[type_path = "sapphire::assets"]
pub enum CueNode {
    /// A sound, by asset ID: a leaf.
    Sound(AssetId),
    /// One of the nodes under it, picked by weight.
    Random(RandomNode),
    /// The nodes under it in turn, one a start, round again after the last.
    Sequence(#[rkyv(omit_bounds)] Vec<CueNode>),
    /// Every node under it, together.
    Layer(#[rkyv(omit_bounds)] Vec<CueNode>),
    /// The node for a named parameter's case, as the start is asked for with: footsteps on a `surface`.
    Switch(SwitchNode),
    /// The two nodes either side of a named parameter's value, crossfaded by equal power as the start is asked
    /// for: the value at the start holds for the voices' lives.
    Blend(BlendNode),
}

/// One of the nodes under it, by weight, keeping clear of its last few picks. See [`CueNode::Random`].
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Writer + rkyv::ser::Allocator, __S::Error: rkyv::rancor::Source),
    deserialize_bounds(__D::Error: rkyv::rancor::Source),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext, __C::Error: rkyv::rancor::Source)),
    derive(Debug)
)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct RandomNode {
    /// What it picks among: at least one.
    #[rkyv(omit_bounds)]
    pub of: Vec<CueNode>,
    /// Each one's chance against the rest's, in order, each at least 1; none, and they're even. A version 2 cue's
    /// sounds are even, so a start picks the one its number's remainder by their count names.
    #[serde(default)]
    pub weights: Vec<u16>,
    /// How many of its last picks it keeps clear of, at most [`MAX_AVOID_REPEAT`] and fewer than it has to pick
    /// from: 1 never picks the same twice running.
    #[serde(default)]
    pub avoid_repeat: u8,
}

/// The node for a named parameter's case. See [`CueNode::Switch`].
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Writer + rkyv::ser::Allocator, __S::Error: rkyv::rancor::Source),
    deserialize_bounds(__D::Error: rkyv::rancor::Source),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext, __C::Error: rkyv::rancor::Source)),
    derive(Debug)
)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct SwitchNode {
    /// The parameter it reads, by name.
    pub on: String,
    /// Its cases, each named once: at least one.
    #[rkyv(omit_bounds)]
    pub cases: Vec<SwitchCase>,
    /// The case it plays when the start names none of its cases, or no value at all: one of them.
    pub default: String,
}

/// One case of a [`SwitchNode`].
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Writer + rkyv::ser::Allocator, __S::Error: rkyv::rancor::Source),
    deserialize_bounds(__D::Error: rkyv::rancor::Source),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext, __C::Error: rkyv::rancor::Source)),
    derive(Debug)
)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct SwitchCase {
    /// The parameter's value it's for: `"grass"`.
    pub when: String,
    #[rkyv(omit_bounds)]
    pub play: CueNode,
}

/// Nodes along a named parameter's values. See [`CueNode::Blend`].
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Writer + rkyv::ser::Allocator, __S::Error: rkyv::rancor::Source),
    deserialize_bounds(__D::Error: rkyv::rancor::Source),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext, __C::Error: rkyv::rancor::Source)),
    derive(Debug)
)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct BlendNode {
    /// The parameter it reads, by name: a number. Asked for without it, a start plays the first point.
    pub on: String,
    /// Where along the parameter each node plays alone, lowest first, each above the last: at least one. Below
    /// the first the first plays alone, and above the last the last.
    #[rkyv(omit_bounds)]
    pub points: Vec<BlendPoint>,
}

/// One point of a [`BlendNode`].
#[derive(
    Reflect,
    Clone,
    Debug,
    PartialEq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(
    serialize_bounds(__S: rkyv::ser::Writer + rkyv::ser::Allocator, __S::Error: rkyv::rancor::Source),
    deserialize_bounds(__D::Error: rkyv::rancor::Source),
    bytecheck(bounds(__C: rkyv::validation::ArchiveContext, __C::Error: rkyv::rancor::Source)),
    derive(Debug)
)]
#[serde(deny_unknown_fields)]
#[type_path = "sapphire::assets"]
pub struct BlendPoint {
    /// The parameter's value where it plays alone.
    pub at: f32,
    #[rkyv(omit_bounds)]
    pub play: CueNode,
}

impl CueNode {
    /// A random pick of `sounds`, evenly: what a version 2 cue's list reads as.
    #[must_use]
    pub fn one_of(sounds: Vec<AssetId>) -> Self {
        Self::Random(RandomNode {
            of: sounds.into_iter().map(Self::Sound).collect(),
            weights: Vec::new(),
            avoid_repeat: 0,
        })
    }

    /// The nodes right under it, in order: none for a sound.
    pub fn children(&self) -> impl Iterator<Item = &Self> {
        let (list, cases, points): (&[Self], &[SwitchCase], &[BlendPoint]) = match self {
            Self::Sound(_) => (&[], &[], &[]),
            Self::Random(random) => (&random.of, &[], &[]),
            Self::Sequence(nodes) | Self::Layer(nodes) => (nodes, &[], &[]),
            Self::Switch(switch) => (&[], &switch.cases, &[]),
            Self::Blend(blend) => (&[], &[], &blend.points),
        };
        list.iter().chain(cases.iter().map(|case| &case.play)).chain(points.iter().map(|point| &point.play))
    }

    /// How many sounds a start can play from it at most, at once.
    #[must_use]
    pub fn most_sounds(&self) -> usize {
        match self {
            Self::Sound(_) => 1,
            Self::Layer(nodes) => nodes.iter().map(Self::most_sounds).sum(),
            // Two neighbours sound at once between points.
            Self::Blend(blend) => {
                let each: Vec<usize> = blend.points.iter().map(|point| point.play.most_sounds()).collect();
                each.windows(2).map(|pair| pair[0] + pair[1]).chain(each.iter().copied()).max().unwrap_or(0)
            }
            Self::Random(_) | Self::Sequence(_) | Self::Switch(_) => {
                self.children().map(Self::most_sounds).max().unwrap_or(0)
            }
        }
    }

    /// Its nodes, it among them, and how deep the deepest is, it at 1.
    fn size(&self) -> (usize, usize) {
        self.children().fold((1, 1), |(nodes, depth), child| {
            let (more, below) = child.size();
            (nodes + more, depth.max(below + 1))
        })
    }

    /// What's wrong with it, if anything, by its path under `at`.
    fn problem(&self, at: &str) -> Option<String> {
        let name_problem = |what: &str, name: &str| {
            (name.is_empty() || name.trim() != name).then(|| format!("{at}: {name:?} isn't {what}"))
        };
        let own = match self {
            Self::Sound(_) => None,
            Self::Random(random) => {
                let count = random.of.len();
                if count == 0 {
                    Some(format!("{at}: a random node picks among at least one"))
                } else if !random.weights.is_empty() && random.weights.len() != count {
                    Some(format!("{at}: {} weights for {count} to pick among", random.weights.len()))
                } else if random.weights.contains(&0) {
                    Some(format!("{at}: a weight of 0 would never be picked"))
                } else if random.avoid_repeat > MAX_AVOID_REPEAT {
                    Some(format!("{at}: avoid_repeat {} is over {MAX_AVOID_REPEAT}", random.avoid_repeat))
                } else if usize::from(random.avoid_repeat) >= count && random.avoid_repeat > 0 {
                    Some(format!(
                        "{at}: avoiding the last {} of {count} would leave nothing to pick",
                        random.avoid_repeat
                    ))
                } else {
                    None
                }
            }
            Self::Sequence(nodes) | Self::Layer(nodes) => {
                nodes.is_empty().then(|| format!("{at}: it plays at least one node"))
            }
            Self::Switch(switch) => name_problem("a parameter's name", &switch.on)
                .or_else(|| switch.cases.is_empty().then(|| format!("{at}: a switch has at least one case")))
                .or_else(|| switch.cases.iter().find_map(|case| name_problem("a case's name", &case.when)))
                .or_else(|| {
                    switch.cases.iter().enumerate().find_map(|(index, case)| {
                        switch.cases[..index]
                            .iter()
                            .any(|earlier| earlier.when == case.when)
                            .then(|| format!("{at}: the case {:?} is named twice", case.when))
                    })
                })
                .or_else(|| {
                    (!switch.cases.iter().any(|case| case.when == switch.default))
                        .then(|| format!("{at}: the default {:?} isn't one of its cases", switch.default))
                }),
            Self::Blend(blend) => name_problem("a parameter's name", &blend.on)
                .or_else(|| blend.points.is_empty().then(|| format!("{at}: a blend has at least one point")))
                .or_else(|| {
                    blend
                        .points
                        .iter()
                        .find(|point| !point.at.is_finite())
                        .map(|point| format!("{at}: {} isn't a value", point.at))
                })
                .or_else(|| {
                    blend
                        .points
                        .windows(2)
                        .find(|pair| pair[1].at <= pair[0].at)
                        .map(|pair| format!("{at}: {} after {} isn't above it", pair[1].at, pair[0].at))
                }),
        };
        own.or_else(|| {
            let path = |index: usize| match self {
                Self::Random(_) => format!("{at}.of[{index}]"),
                Self::Sequence(_) | Self::Layer(_) => format!("{at}[{index}]"),
                Self::Switch(_) => format!("{at}.cases[{index}].play"),
                Self::Blend(_) => format!("{at}.points[{index}].play"),
                Self::Sound(_) => at.to_owned(),
            };
            self.children().enumerate().find_map(|(index, child)| child.problem(&path(index)))
        })
    }

    /// Adds each sound under it to `sounds`, the first time it's met.
    fn sounds_into(&self, sounds: &mut Vec<AssetId>) {
        match self {
            Self::Sound(sound) if !sounds.contains(sound) => sounds.push(*sound),
            _ => self.children().for_each(|child| child.sounds_into(sounds)),
        }
    }
}

/// How many voices may sound at once, and which goes when one more is asked for (M6 U3): Unreal's
/// concurrency rules.
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
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct Limit {
    /// Voices at once, at least 1 and at most [`MAX_LIMIT`].
    pub max: u16,
    pub rule: LimitRule,
}

impl Limit {
    /// What's wrong with the limit, if anything.
    #[must_use]
    pub fn problem(&self) -> Option<String> {
        (!(1..=MAX_LIMIT).contains(&self.max))
            .then(|| format!("a limit of {} voices isn't from 1 to {MAX_LIMIT}", self.max))
    }
}

/// Which voice goes when a [`Limit`] is reached and one more is asked for.
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
#[rkyv(derive(Debug))]
pub enum LimitRule {
    /// The new one doesn't play: what's sounding is never cut.
    RefuseNew,
    /// The one that started first ends, and the new one plays.
    StopOldest,
    /// The quietest at the listener ends, the new one among them: if it's the quietest, it doesn't play.
    StopQuietest,
    /// The lowest priority ends, the oldest of them on a tie, the new one among them: if it's lower than all,
    /// it doesn't play.
    StopLowestPriority,
}

/// How a placed cue fades with distance.
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
#[rkyv(derive(Debug))]
#[serde(deny_unknown_fields)]
pub struct Attenuation {
    /// Meters out to which the cue plays at full volume.
    pub near: f32,
    /// Meters at and beyond which it is silent.
    pub far: f32,
    /// The fade between them.
    #[serde(default)]
    pub curve: AttenuationCurve,
}

impl Attenuation {
    /// The gain at `distance` meters, in decibels: 0 out to [`Self::near`], [`SILENCE_DB`] from
    /// [`Self::far`] on, falling all the way between.
    pub fn gain_db(&self, distance: f32) -> f32 {
        if distance <= self.near {
            return 0.0;
        }
        if distance >= self.far {
            return SILENCE_DB;
        }
        let along = (distance - self.near) / (self.far - self.near);
        match self.curve {
            AttenuationCurve::Linear => SILENCE_DB * along,
            // The inverse distance law, then down to silence over the range's last quarter, so a
            // cue with a far `far` doesn't cut out while still audible.
            AttenuationCurve::Inverse => {
                // Held to silence: a far `far` would otherwise climb back up it, and a `near` of 0, which
                // reads as infinitely quiet, would come out of the sum below as NaN.
                let law = (20.0 * (self.near / distance).log10()).max(SILENCE_DB);
                let fade = ((along - 0.75) / 0.25).clamp(0.0, 1.0);
                law + (SILENCE_DB - law) * fade
            }
        }
    }
}

/// See [`Attenuation::gain_db`].
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
pub enum AttenuationCurve {
    /// Evenly in decibels from full to silent: a designer's fade.
    Linear,
    /// By the inverse distance law, as sound falls off in the open: most of the fall is close by.
    #[default]
    Inverse,
}

/// A mixer bus. The world's buses follow the world clock's snapshot; the interface, music and voice don't.
#[derive(
    Reflect,
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
#[reflect(Serialize, Deserialize)]
#[rkyv(derive(Debug))]
pub enum Bus {
    /// Combat and everything else that happens in the world.
    #[default]
    Sfx,
    /// The world's beds and details.
    Ambience,
    /// The interface: heard the same whatever the world's clock does.
    Ui,
    /// A score's stems ([`Score`](crate::score::Score), M3 A1), which go down under a voice line.
    Music,
    /// Voice lines ([`VoiceLines`](crate::voice::VoiceLines), M3 A1), in the voice language.
    Voice,
}

impl Bus {
    /// Every bus, in a stable order. New buses join at the end.
    pub const ALL: [Self; 5] = [Self::Sfx, Self::Ambience, Self::Ui, Self::Music, Self::Voice];

    /// The bus's position in [`Self::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The category's name in a [`Mix`](crate::mix::Mix) (M6 U2): the root of its buses.
    pub const fn mix_name(self) -> &'static str {
        match self {
            Self::Sfx => "sfx",
            Self::Ambience => "ambience",
            Self::Ui => "ui",
            Self::Music => "music",
            Self::Voice => "voice",
        }
    }

    /// The category a mix's root bus is named for.
    pub fn from_mix_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|bus| bus.mix_name() == name)
    }
}

/// What a voice does while the actor it is attached to is in hitstop.
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
#[rkyv(derive(Debug))]
pub enum HitstopResponse {
    /// Plays on: the hit itself.
    #[default]
    Ignore,
    /// Plays on this many decibels quieter (not above 0): what shouldn't mask the hit.
    Duck(f32),
    /// Stops where it is and goes on when the hitstop ends: a swing that froze with the arm.
    Freeze,
}

/// A cue that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum CueError {
    /// The document isn't a cue document at all: the wrong type or a version out of reach.
    #[error(transparent)]
    Document(Box<DocumentError>),
    /// Malformed RON, or a value of the wrong kind. `text` is the document's own line, which names
    /// the field.
    #[error("line {line}, column {column}: {message}: `{text}`")]
    Malformed { line: usize, column: usize, message: String, text: String },
    /// A value that parsed but can't be played.
    #[error("`{field}`: {problem}")]
    Field { field: &'static str, problem: String },
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

impl Cue {
    /// Reads and checks a cue document.
    pub fn from_document(text: &str) -> Result<Self, CueError> {
        let cue = sapphire_serialize::from_ron::<Self>(text).map_err(|error| match error {
            DocumentError::Parse(spanned) => {
                let line = spanned.span.start.line;
                let quoted = text.lines().nth(line.saturating_sub(1)).unwrap_or_default().trim();
                CueError::Malformed {
                    line,
                    column: spanned.span.start.col,
                    message: spanned.code.to_string(),
                    text: quoted.to_owned(),
                }
            }
            other => CueError::Document(Box::new(other)),
        })?;
        cue.check()?;
        Ok(cue)
    }

    /// Checks that every value can be played, naming the first that can't.
    pub fn check(&self) -> Result<(), CueError> {
        let field = |field, problem: String| Err(CueError::Field { field, problem });
        let (nodes, depth) = self.play.size();
        if nodes > MAX_TREE_NODES {
            return field("play", format!("{nodes} nodes are over the {MAX_TREE_NODES} a cue may have"));
        }
        if depth > MAX_TREE_DEPTH {
            return field("play", format!("{depth} deep is over the {MAX_TREE_DEPTH} a cue may go"));
        }
        if let Some(problem) = self.play.problem("play") {
            return field("play", problem);
        }
        let most = self.play.most_sounds();
        if most > MAX_START_SOUNDS {
            return field(
                "play",
                format!("a start could play {most} sounds at once, over the {MAX_START_SOUNDS} it may"),
            );
        }
        let [quiet, loud] = self.volume_db;
        if !(quiet.is_finite() && loud.is_finite()) || quiet > loud {
            return field("volume_db", format!("({quiet}, {loud}) isn't a range, least to most"));
        }
        if loud > MAX_VOLUME_DB {
            return field(
                "volume_db",
                format!("{loud} dB is above the {MAX_VOLUME_DB} dB a cue may ask for"),
            );
        }
        let [slow, fast] = self.pitch;
        if !(slow.is_finite() && fast.is_finite()) || slow > fast {
            return field("pitch", format!("({slow}, {fast}) isn't a range, least to most"));
        }
        if !(PITCH_RANGE.contains(&slow) && PITCH_RANGE.contains(&fast)) {
            return field(
                "pitch",
                format!("({slow}, {fast}) leaves {} to {}", PITCH_RANGE.start(), PITCH_RANGE.end()),
            );
        }
        if let Some(Attenuation { near, far, curve }) = self.attenuation {
            if !near.is_finite() || near < 0.0 {
                return field("attenuation.near", format!("{near} isn't a distance"));
            }
            if !far.is_finite() || far <= near {
                return field("attenuation.far", format!("{far} isn't beyond `near`, {near}"));
            }
            if near == 0.0 && curve == AttenuationCurve::Inverse {
                return field(
                    "attenuation.near",
                    "the inverse law falls from `near`, so 0 would leave nothing to hear".to_owned(),
                );
            }
        }
        if let HitstopResponse::Duck(db) = self.hitstop
            && !(db.is_finite() && db <= 0.0)
        {
            return field("hitstop", format!("Duck({db}) isn't quieter"));
        }
        if let Some(problem) = self.limit.as_ref().and_then(Limit::problem) {
            return field("limit.max", problem);
        }
        if let Some(group) = &self.group
            && (group.is_empty() || group.trim() != group)
        {
            return field("group", format!("{group:?} isn't a group's name"));
        }
        Ok(())
    }

    /// Every sound its tree can play, each once, in the order the tree names them: what it depends on.
    #[must_use]
    pub fn sounds(&self) -> Vec<AssetId> {
        let mut sounds = Vec::new();
        self.play.sounds_into(&mut sounds);
        sounds
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedCue, CueError> {
        let cue = sapphire_serialize::access::<Self>(bytes)?;
        cue.to_owned_cue().check()?;
        Ok(cue)
    }
}

impl ArchivedCue {
    /// The cue as plain data.
    #[must_use]
    pub fn to_owned_cue(&self) -> Cue {
        rkyv::deserialize::<Cue, rkyv::rancor::Error>(self)
            .expect("an archive that passed its check deserializes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOUND: &str = "4f6b2c1e-8a3d-4e5f-9b7c-0d1e2f3a4b5c";

    /// A cue document of the current version with these lines in its data.
    fn document(data: &str) -> String {
        versioned(3, data)
    }

    fn versioned(version: u32, data: &str) -> String {
        format!(
            "(\n    type: \"sapphire::assets::Cue\",\n    version: {version},\n    data: (\n{data}\n    ),\n)\n"
        )
    }

    fn sounds() -> String {
        format!("        play: Sound(\"{SOUND}\"),")
    }

    fn id(n: u8) -> AssetId {
        AssetId::from_bytes([n; 16])
    }

    /// What reading a document with these lines after `play` says.
    fn error_of(lines: &str) -> String {
        let text = document(&format!("{}\n{lines}", sounds()));
        Cue::from_document(&text).expect_err("a malformed cue").to_string()
    }

    #[test]
    fn a_whole_document_reads_as_written() {
        let text = document(&format!(
            "{}
        volume_db: (-6.0, -3.0),
        pitch: (0.95, 1.05),
        attenuation: Some((near: 1.0, far: 30.0, curve: Linear)),
        bus: Ambience,
        hitstop: Duck(-12.0),",
            sounds()
        ));
        let cue = Cue::from_document(&text).unwrap();
        assert_eq!(
            cue,
            Cue {
                play: CueNode::Sound(SOUND.parse().unwrap()),
                volume_db: [-6.0, -3.0],
                pitch: [0.95, 1.05],
                attenuation: Some(Attenuation { near: 1.0, far: 30.0, curve: AttenuationCurve::Linear }),
                bus: Bus::Ambience,
                hitstop: HitstopResponse::Duck(-12.0),
                priority: DEFAULT_PRIORITY,
                limit: None,
                group: None,
                looped: false,
            }
        );
    }

    #[test]
    fn only_what_it_plays_is_required() {
        let cue = Cue::from_document(&document(&sounds())).unwrap();
        assert_eq!(cue.volume_db, [0.0, 0.0]);
        assert_eq!(cue.pitch, [1.0, 1.0]);
        assert_eq!(cue.attenuation, None);
        assert_eq!((cue.bus, cue.hitstop), (Bus::Sfx, HitstopResponse::Ignore));
        assert_eq!((cue.priority, cue.limit, cue.group, cue.looped), (DEFAULT_PRIORITY, None, None, false));
    }

    /// M6 U3: the fields of how a cue competes read as written and cook back.
    #[test]
    fn a_cue_says_how_it_competes() {
        let text = document(&format!(
            "{}
        priority: 200,
        limit: Some((max: 3, rule: StopQuietest)),
        group: Some(\"footsteps\"),
        looped: true,",
            sounds()
        ));
        let cue = Cue::from_document(&text).unwrap();
        assert_eq!(cue.priority, 200);
        assert_eq!(cue.limit, Some(Limit { max: 3, rule: LimitRule::StopQuietest }));
        assert_eq!((cue.group.as_deref(), cue.looped), (Some("footsteps"), true));
        let cooked = cue.to_cooked().unwrap();
        assert_eq!(Cue::read(&cooked).unwrap().to_owned_cue(), cue);
    }

    /// M6 U4a: a version 2 cue's list, and a version 1's, read as a random node of equal weights, with everything
    /// else as written; neither knows `play`, version 1 knows nothing of competing, and no later version is read.
    #[test]
    fn older_versions_read_their_sounds_as_an_even_random_node() {
        let list = format!("        sounds: [\"{}\", \"{}\"],", id(1), id(2));
        let even = CueNode::one_of(vec![id(1), id(2)]);
        let second = Cue::from_document(&versioned(2, &format!("{list}\n        priority: 7,"))).unwrap();
        assert_eq!(second, Cue { play: even.clone(), priority: 7, ..Cue::default() });
        let first =
            Cue::from_document(&versioned(1, &format!("{list}\n        volume_db: (-6.0, -3.0),"))).unwrap();
        assert_eq!(first, Cue { play: even, volume_db: [-6.0, -3.0], ..Cue::default() });

        let error = Cue::from_document(&versioned(1, &format!("{list}\n        priority: 3,"))).unwrap_err();
        assert!(error.to_string().contains("priority"), "version 1 had no priority: {error}");
        let error = Cue::from_document(&versioned(2, &sounds())).unwrap_err().to_string();
        assert!(error.contains("play"), "version 2 had no tree: {error}");
        let error = Cue::from_document(&versioned(3, &list)).unwrap_err().to_string();
        assert!(error.contains("sounds"), "version 3 has no list: {error}");
        let error = Cue::from_document(&versioned(4, &sounds())).unwrap_err();
        assert!(matches!(&error, CueError::Document(_)), "{error}");
    }

    /// Every node reads as written, cooks and reads back equal, and names its sounds once each.
    #[test]
    fn a_tree_reads_cooks_and_names_its_sounds() {
        let text = document(&format!(
            "        play: Layer([
            Switch((
                on: \"surface\",
                cases: [
                    (when: \"stone\", play: Random((of: [Sound(\"{a}\"), Sound(\"{b}\")], weights: [3, 1], avoid_repeat: 1))),
                    (when: \"grass\", play: Sequence([Sound(\"{c}\"), Sound(\"{a}\")])),
                ],
                default: \"stone\",
            )),
            Blend((on: \"speed\", points: [(at: 0.0, play: Sound(\"{d}\")), (at: 5.0, play: Sound(\"{b}\"))])),
        ]),",
            a = id(1),
            b = id(2),
            c = id(3),
            d = id(4),
        ));
        let cue = Cue::from_document(&text).unwrap();
        let CueNode::Layer(layers) = &cue.play else { panic!("{:?}", cue.play) };
        let CueNode::Switch(switch) = &layers[0] else { panic!("{:?}", layers[0]) };
        assert_eq!(
            (switch.on.as_str(), switch.default.as_str(), switch.cases.len()),
            ("surface", "stone", 2)
        );
        let CueNode::Random(random) = &switch.cases[0].play else { panic!("{:?}", switch.cases[0]) };
        assert_eq!((random.weights.as_slice(), random.avoid_repeat), ([3, 1].as_slice(), 1));
        assert_eq!(cue.sounds(), [id(1), id(2), id(3), id(4)]);
        // A switch or random plays one under it, a layer all, a blend two neighbours.
        assert_eq!(cue.play.most_sounds(), 3);
        let cooked = cue.to_cooked().unwrap();
        assert_eq!(Cue::read(&cooked).unwrap().to_owned_cue(), cue);
    }

    /// Each rule of the tree, by the path of the node that breaks it.
    #[test]
    fn a_tree_that_cant_be_played_says_where() {
        let sound = |n| format!("Sound(\"{}\")", id(n));
        let (a, b) = (sound(1), sound(2));
        let rows: Vec<(String, &[&str])> = vec![
            ("Random((of: []))".to_owned(), &["play:", "at least one"]),
            (format!("Random((of: [{a}, {b}], weights: [1]))"), &["1 weights for 2"]),
            (format!("Random((of: [{a}, {b}], weights: [1, 0]))"), &["weight of 0"]),
            (format!("Random((of: [{a}, {b}], avoid_repeat: 2))"), &["last 2 of 2"]),
            (format!("Random((of: [{a}, {b}], avoid_repeat: 9))"), &["avoid_repeat 9"]),
            ("Layer([])".to_owned(), &["at least one node"]),
            (format!("Sequence([{a}, Layer([])])"), &["play[1]", "at least one node"]),
            (
                format!(
                    "Switch((on: \"surface\", cases: [(when: \"stone\", play: {a})], default: \"sand\"))"
                ),
                &["default \"sand\""],
            ),
            (
                format!(
                    "Switch((on: \" surface\", cases: [(when: \"stone\", play: {a})], default: \"stone\"))"
                ),
                &["parameter's name"],
            ),
            (
                format!(
                    "Switch((on: \"surface\", cases: [(when: \"stone\", play: {a}), (when: \"stone\", play: {b})], \
                     default: \"stone\"))"
                ),
                &["named twice"],
            ),
            (
                format!("Switch((on: \"surface\", cases: [(when: \"\", play: {a})], default: \"\"))"),
                &["case's name"],
            ),
            ("Switch((on: \"surface\", cases: [], default: \"stone\"))".to_owned(), &["at least one case"]),
            (
                format!("Blend((on: \"speed\", points: [(at: 2.0, play: {a}), (at: 1.0, play: {b})]))"),
                &["1 after 2"],
            ),
            (format!("Blend((on: \"speed\", points: [(at: inf, play: {a})]))"), &["inf isn't a value"]),
            ("Blend((on: \"speed\", points: []))".to_owned(), &["at least one point"]),
            (
                format!(
                    "Random((of: [Sound(\"{}\"), Switch((on: \"s\", cases: [(when: \"x\", play: Layer([]))], default: \"x\"))]))",
                    id(1)
                ),
                &["play.of[1].cases[0].play", "at least one node"],
            ),
            (format!("Layer([{}])", vec![a.clone(); MAX_START_SOUNDS + 1].join(", ")), &["9 sounds at once"]),
            (
                format!(
                    "Blend((on: \"v\", points: [(at: 0.0, play: Layer([{a}, {a}, {a}, {a}, {a}])), (at: 1.0, play: Layer([{b}, {b}, {b}, {b}]))]))"
                ),
                &["9 sounds at once"],
            ),
            ((0..MAX_TREE_DEPTH).fold(a.clone(), |inner, _| format!("Layer([{inner}])")), &["9 deep"]),
        ];
        let mut failures = Vec::new();
        for (play, expected) in &rows {
            let text = document(&format!("        play: {play},"));
            match Cue::from_document(&text) {
                Ok(_) => failures.push(format!("{play} read")),
                Err(error) => {
                    let error = error.to_string();
                    if !expected.iter().all(|word| error.contains(word)) {
                        failures.push(format!("{play} said {error:?}, which lacks one of {expected:?}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
        let deepest = (1..MAX_TREE_DEPTH).fold(a, |inner, _| format!("Layer([{inner}])"));
        Cue::from_document(&document(&format!("        play: {deepest},"))).expect("as deep as a cue may go");
    }

    /// Whatever the document says, a gain is a number, and it only ever falls with distance.
    #[test]
    fn the_gain_falls_from_near_to_far_and_stays_a_number() {
        let curves = [AttenuationCurve::Linear, AttenuationCurve::Inverse];
        for (near, far) in [(0.0, 30.0), (0.001, 30.0), (1.0, 30.0), (0.1, 10_000.0), (4.0, 4.5)] {
            for curve in curves {
                let attenuation = Attenuation { near, far, curve };
                let mut last = attenuation.gain_db(0.0);
                assert_eq!(last, 0.0, "{attenuation:?}");
                for step in 1..=400 {
                    let distance = far * step as f32 / 300.0;
                    let gain = attenuation.gain_db(distance);
                    assert!(gain.is_finite(), "{attenuation:?} at {distance} m: {gain}");
                    assert!(gain <= last + 1e-3, "{attenuation:?} at {distance} m: {last}, then {gain}");
                    assert!(gain >= SILENCE_DB, "{attenuation:?} at {distance} m: {gain}");
                    last = gain;
                }
                assert_eq!(last, SILENCE_DB, "{attenuation:?}");
            }
        }
    }

    #[test]
    fn an_inverse_fall_needs_somewhere_to_fall_from() {
        let error = error_of("        attenuation: Some((near: 0.0, far: 30.0)),");
        assert!(error.contains("attenuation.near"), "{error}");
        let text = document(&format!(
            "{}\n        attenuation: Some((near: 0.0, far: 30.0, curve: Linear)),",
            sounds()
        ));
        Cue::from_document(&text).expect("a designer's fade starts anywhere");
    }

    #[test]
    fn a_cooked_cue_reads_back_equal_and_names_its_sounds() {
        let cue = Cue::from_document(&document(&format!(
            "{}\n        attenuation: Some((near: 2.0, far: 8.0)),\n        hitstop: Freeze,",
            sounds()
        )))
        .unwrap();
        let cooked = cue.to_cooked().unwrap();
        assert_eq!(Cue::read(&cooked).unwrap().to_owned_cue(), cue);
    }

    #[test]
    fn what_a_read_refuses_is_refused_cooked_too() {
        let mut cue = Cue::from_document(&document(&sounds())).unwrap();
        cue.volume_db = [3.0, 1.0];
        let cooked = cue.to_cooked().unwrap();
        assert!(matches!(Cue::read(&cooked), Err(CueError::Field { field: "volume_db", .. })));
    }

    #[test]
    fn malformed_cues_name_the_bad_field() {
        // Each row: lines that break the document, and what the error must say.
        let rows: &[(&str, &[&str])] = &[
            // Fields that don't exist, and values of the wrong kind or variant.
            ("        volum_db: (-1.0, 0.0),", &["volum_db"]),
            ("        volume_db: \"loud\",", &["volume_db", "line 6"]),
            ("        pitch: 1.0,", &["pitch"]),
            ("        bus: Choir,", &["bus", "Choir"]),
            ("        hitstop: Duck(\"lots\"),", &["hitstop"]),
            ("        attenuation: Some((near: 1.0)),", &["attenuation", "far"]),
            // Values that parse but can't be played.
            ("        volume_db: (3.0, 1.0),", &["volume_db"]),
            ("        volume_db: (NaN, 1.0),", &["volume_db"]),
            ("        volume_db: (0.0, 100.0),", &["volume_db"]),
            ("        pitch: (1.2, 0.8),", &["pitch"]),
            ("        pitch: (0.0, 1.0),", &["pitch"]),
            ("        pitch: (1.0, inf),", &["pitch"]),
            ("        attenuation: Some((near: -1.0, far: 3.0)),", &["attenuation.near"]),
            ("        attenuation: Some((near: 5.0, far: 3.0)),", &["attenuation.far"]),
            ("        attenuation: Some((near: 1.0, far: inf)),", &["attenuation.far"]),
            ("        hitstop: Duck(6.0),", &["hitstop"]),
            ("        hitstop: Duck(NaN),", &["hitstop"]),
            ("        limit: Some((max: 0, rule: RefuseNew)),", &["limit.max"]),
            ("        limit: Some((max: 600, rule: StopOldest)),", &["limit.max"]),
            ("        limit: Some((max: 2)),", &["limit", "rule"]),
            ("        limit: Some((max: 2, rule: StopNewest)),", &["StopNewest"]),
            ("        group: Some(\"\"),", &["group"]),
            ("        group: Some(\" steps\"),", &["group"]),
            ("        priority: 256,", &["priority"]),
        ];
        let mut failures = Vec::new();
        for (lines, expected) in rows {
            let error = error_of(lines);
            if !expected.iter().all(|word| error.contains(word)) {
                failures.push(format!("{lines:?} said {error:?}, which lacks one of {expected:?}"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_missing_or_empty_tree_says_so() {
        let error = Cue::from_document(&document("        bus: Ui,")).unwrap_err().to_string();
        assert!(error.contains("play"), "{error}");
        let error = Cue::from_document(&document("        play: Random((of: [])),")).unwrap_err().to_string();
        assert!(error.contains("play"), "{error}");
        let error =
            Cue::from_document(&document("        play: Sound(\"not-an-id\"),")).unwrap_err().to_string();
        assert!(error.contains("play"), "{error}");
        let error = Cue::from_document(&document("        play: Chorus([]),")).unwrap_err().to_string();
        assert!(error.contains("Chorus"), "{error}");
    }

    #[test]
    fn a_document_of_another_type_is_not_a_cue() {
        let text = "(type: \"sapphire::assets::Other\", version: 1, data: ())";
        let error = Cue::from_document(text).unwrap_err();
        assert!(
            matches!(&error, CueError::Document(error) if matches!(**error, DocumentError::WrongType { .. }))
        );
    }

    #[test]
    fn attenuation_falls_from_full_to_silence() {
        for curve in [AttenuationCurve::Linear, AttenuationCurve::Inverse] {
            let attenuation = Attenuation { near: 2.0, far: 40.0, curve };
            assert_eq!(attenuation.gain_db(0.0), 0.0);
            assert_eq!(attenuation.gain_db(2.0), 0.0);
            assert_eq!(attenuation.gain_db(40.0), SILENCE_DB);
            assert_eq!(attenuation.gain_db(1000.0), SILENCE_DB);
            let mut last = 0.0;
            for meters in 3..40 {
                let gain = attenuation.gain_db(meters as f32);
                assert!(gain < last, "{curve:?} at {meters} m: {gain} after {last}");
                assert!(gain > SILENCE_DB, "{curve:?} at {meters} m: {gain}");
                last = gain;
            }
        }
        // The inverse law: every doubling of the distance is 6 dB, until the fade at the far end.
        let attenuation = Attenuation { near: 1.0, far: 100.0, curve: AttenuationCurve::Inverse };
        assert!((attenuation.gain_db(2.0) + 6.02).abs() < 0.01);
        assert!((attenuation.gain_db(16.0) + 24.08).abs() < 0.01);
    }

    #[test]
    fn each_category_is_named_in_a_mix_and_keeps_its_place() {
        for bus in Bus::ALL {
            assert_eq!(Bus::from_mix_name(bus.mix_name()), Some(bus));
        }
        assert_eq!(Bus::from_mix_name("Sfx"), None);
        for (index, bus) in Bus::ALL.into_iter().enumerate() {
            assert_eq!(bus.index(), index);
        }
    }
}
