//! The mix (`.mix.ron` documents, cooked to `.smix`; M6 U2, spec §5.15): the buses a game's sounds play
//! through, what each does to them, and how the mix changes with the game.
//!
//! ```ron
//! (
//!     type: "sapphire::assets::Mix",
//!     version: 3,
//!     data: (
//!         // The five categories first, in this order; a bus under one is named by its path.
//!         buses: [
//!             (name: "sfx", room_db: Some(-12.0)),
//!             (name: "ambience"),
//!             (name: "ui"),
//!             (name: "music"),
//!             (name: "voice"),
//!             (name: "sfx/hits", volume_db: -2.0, effects: [HighPass(cutoff_hz: 80.0)]),
//!         ],
//!         // M6 U5: the reverbs a zone's rooms name.
//!         reverbs: [(name: "shelter", feedback: 0.8, damping: 0.3, stereo_width: 1.0)],
//!         snapshots: [
//!             (
//!                 name: "slow_motion",
//!                 priority: 10,
//!                 attack: 0.03,
//!                 release: 0.25,
//!                 driver: WorldTimeScale(floor: 0.2),
//!                 buses: [(bus: "sfx", low_pass_hz: Some(900.0), pitch: Some(0.75))],
//!             ),
//!             (name: "pause", attack: 0.05, release: 0.2, buses: [(bus: "sfx", pause: true)]),
//!         ],
//!         ducking: [
//!             (when: "voice", follow: Envelope(full_at: 0.5), attack: 0.02, release: 0.03,
//!              duck: [(bus: "music", gain_db: -10.0)]),
//!         ],
//!         profiles: [(profile: Home), (profile: Portable, effects: [Compressor(threshold_db: -18.0, ratio: 3.0, attack_ms: 10.0, release_ms: 120.0, makeup_db: 4.0)])],
//!         // M6 U3: kinds of sound sharing a limit, which cues name.
//!         groups: [(name: "footsteps", limit: (max: 6, rule: StopOldest))],
//!     ),
//! )
//! ```
//!
//! - **Buses** make a tree under the five categories a cue names ([`Bus`]): `sfx`, `ambience`, `ui`, `music`
//!   and `voice` are its roots, the first five buses in that order, and a bus below one is named by its path,
//!   its parent listed before it. Each has a volume and up to [`MAX_EFFECTS`] effects in order, and a category
//!   its voices' level to the reverb of the room the listener stands in (`room_db`, version 3). Voices play on
//!   the categories, so a bus below one may not name a level of its own.
//! - **Reverbs** (version 3, M6 U5) are kira's Freeverb, by name, at most [`MAX_REVERBS`]: what a zone's rooms
//!   sound like. The room the listener stands in plays its reverb, crossfaded from the last on a change, and a
//!   voice reaches it only from the same space.
//! - **Snapshots** are offsets to buses, with an attack and a release (seconds for a change to go about two
//!   thirds of the way) and a priority. A snapshot is in by how far the game asks for it: wholly while it's
//!   asked by name, or by how far the world's clock has slowed toward a floor
//!   ([`SnapshotDriver::WorldTimeScale`]). Gains add; a low-pass or a pitch goes from what the snapshots below
//!   it in priority made toward its own as far as it's in. Pitch bends the voices of the bus and those under
//!   it, and `pause` holds them where they are, tails and all.
//! - **Ducking rules** take buses down while a bus's voices sound: by the cooked mouth envelope of a voice
//!   line at its playhead ([`DuckFollow::Envelope`]), or while any plays.
//! - **Output profiles** are the master's volume and effects for where the sound is heard ([`OutputProfile`]).
//! - **Voice groups** (version 2, M6 U3) are kinds of sound sharing a [`Limit`] on how many play at once, by
//!   Unreal's rules, whichever cues name them. A version 1 mix reads as written, with none.
//!
//! Versions 1 and 2 had two fixed reverb sends and each bus's levels to them, which version 3's rooms replace: an
//! older mix reads as written if it has none, and is refused, naming them, if it has any.
//!
//! [`Mix::from_document`] says what's wrong by field path, or by the document's own line. The cooked form is
//! the same type. Making the buses, moving the snapshots and following the envelopes are `sapphire_audio`'s.

use bevy_reflect::{Reflect, ReflectDeserialize, ReflectSerialize};
use sapphire_reflect::{Decoder, VersionError, Versioned};
use sapphire_serialize::cooked::{AlignedVec, CookedError};
use serde::{Deserialize, Serialize};

use crate::cue::{Bus, Limit, MAX_VOLUME_DB, SILENCE_DB};
use crate::score::{DocumentProblem, read_document};

/// Conventional file extension for cooked mixes.
pub const MIX_EXTENSION: &str = "smix";

/// The most buses a mix may have, the five categories among them.
pub const MAX_BUSES: usize = 64;

/// The most effects one bus or profile may have.
pub const MAX_EFFECTS: usize = 4;

/// The most reverbs a mix may have (M6 U5).
pub const MAX_REVERBS: usize = 32;

/// The most snapshots a mix may have.
pub const MAX_SNAPSHOTS: usize = 32;

/// The most ducking rules a mix may have.
pub const MAX_RULES: usize = 16;

/// The most voice groups a mix may have.
pub const MAX_GROUPS: usize = 32;

/// The frequencies a filter or a band may sit at, in hertz.
const HEARD_HZ: std::ops::RangeInclusive<f32> = 20.0..=20_000.0;

/// The pitches a snapshot may bend a bus to.
const PITCH_RANGE: std::ops::RangeInclusive<f32> = 0.25..=4.0;

/// The most seconds a snapshot or a rule may take to move.
const MAX_SECONDS: f32 = 10.0;

/// A game's mix. See the [module docs](self).
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
pub struct Mix {
    /// The bus tree, each parent before its children: the five categories and what's under them.
    pub buses: Vec<MixBus>,
    /// What rooms sound like (M6 U5): the reverbs a zone's rooms name.
    #[serde(default)]
    pub reverbs: Vec<Reverb>,
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
    #[serde(default)]
    pub ducking: Vec<DuckRule>,
    /// One for each place the sound is heard, `Home` among them.
    pub profiles: Vec<Profile>,
    /// Voice groups (M6 U3): kinds of sound that share a limit on how many play at once, whichever cues they come
    /// from ([`Cue::group`](crate::cue::Cue::group)).
    #[serde(default)]
    pub groups: Vec<VoiceGroup>,
}

impl Versioned for Mix {
    /// Version 2 (M6 U3): `groups`, none in a version 1 mix. Version 3 (M6 U5): `reverbs` and each bus's `room_db`
    /// in place of two fixed sends and each bus's levels to them, which an older mix may not have.
    const VERSION: u32 = 3;

    fn upgrade<D: Decoder>(version: u32, data: D) -> Result<Self, VersionError<D::Error>> {
        match version {
            1 => data.decode::<older::MixV1>().map(Into::into).map_err(VersionError::Decode),
            2 => data.decode::<older::MixV2>().map(Into::into).map_err(VersionError::Decode),
            found => Err(VersionError::TooOld { found, oldest: 1 }),
        }
    }
}

/// Versions 1 (M6 U2) and 2 (M6 U3), as their documents read: version 3's fields but the reverbs and each bus's
/// `room_db`, with fixed sends instead, and in version 1 no groups. A mix with a send, or a bus sending to one, is
/// refused as it's read: a fixed reverb has no room to play in, and dropping it would change what the mix sounds
/// like.
mod older {
    use serde::Deserialize;
    use serde::de::{Error, IgnoredAny};

    use super::{BusEffect, DuckRule, MixBus, Profile, Snapshot, VoiceGroup};

    /// Why an older mix with sends doesn't read.
    const NO_SENDS: &str = "a mix's fixed `sends` are gone since version 3: name `reverbs` for rooms, and give a \
                            bus a `room_db`";

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Bus {
        name: String,
        #[serde(default)]
        volume_db: f32,
        #[serde(default)]
        effects: Vec<BusEffect>,
        #[serde(default, deserialize_with = "no_sends")]
        #[expect(dead_code, reason = "read only to refuse any")]
        sends: (),
    }

    /// An older document's sends, which must be none.
    fn no_sends<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<(), D::Error> {
        let sends: Vec<IgnoredAny> = Vec::deserialize(deserializer)?;
        if sends.is_empty() { Ok(()) } else { Err(D::Error::custom(NO_SENDS)) }
    }

    impl From<Bus> for MixBus {
        fn from(Bus { name, volume_db, effects, .. }: Bus) -> Self {
            Self { name, volume_db, effects, room_db: None }
        }
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct MixV1 {
        buses: Vec<Bus>,
        #[serde(default, deserialize_with = "no_sends")]
        #[expect(dead_code, reason = "read only to refuse any")]
        sends: (),
        #[serde(default)]
        snapshots: Vec<Snapshot>,
        #[serde(default)]
        ducking: Vec<DuckRule>,
        profiles: Vec<Profile>,
    }

    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    pub(super) struct MixV2 {
        buses: Vec<Bus>,
        #[serde(default, deserialize_with = "no_sends")]
        #[expect(dead_code, reason = "read only to refuse any")]
        sends: (),
        #[serde(default)]
        snapshots: Vec<Snapshot>,
        #[serde(default)]
        ducking: Vec<DuckRule>,
        profiles: Vec<Profile>,
        #[serde(default)]
        groups: Vec<VoiceGroup>,
    }

    impl From<MixV1> for super::Mix {
        fn from(MixV1 { buses, snapshots, ducking, profiles, .. }: MixV1) -> Self {
            MixV2 { buses, sends: (), snapshots, ducking, profiles, groups: Vec::new() }.into()
        }
    }

    impl From<MixV2> for super::Mix {
        fn from(MixV2 { buses, snapshots, ducking, profiles, groups, .. }: MixV2) -> Self {
            Self {
                buses: buses.into_iter().map(Into::into).collect(),
                reverbs: Vec::new(),
                snapshots,
                ducking,
                profiles,
                groups,
            }
        }
    }
}

/// A kind of sound whose voices share a limit, whichever cues they come from (M6 U3): footsteps, say, or
/// the crowd's barks.
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
pub struct VoiceGroup {
    /// What a cue names it by.
    pub name: String,
    pub limit: Limit,
}

/// One bus.
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
pub struct MixBus {
    /// A category's name ([`Bus::mix_name`]), or a path under one: `sfx/hits`.
    pub name: String,
    /// Its gain, in decibels.
    #[serde(default)]
    pub volume_db: f32,
    /// What it does to what plays through it, in order.
    #[serde(default)]
    pub effects: Vec<BusEffect>,
    /// Its voices' level to the reverb of the room the listener stands in, in decibels, from each voice as it's
    /// heard (M6 U5): taken before this bus's effects and snapshot filter, and moving with its gain. A voice reaches
    /// the reverb only from the listener's space ([`Reverb`]). Only a category's: voices play on the categories,
    /// so a bus below one with a level is refused. None sends nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_db: Option<f32>,
}

/// An effect on a bus or a profile: kira's own.
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
pub enum BusEffect {
    /// Takes out what's above the cutoff, 12 dB an octave.
    LowPass { cutoff_hz: f32 },
    /// Takes out what's below the cutoff, 12 dB an octave.
    HighPass { cutoff_hz: f32 },
    /// Raises or lowers a band.
    Eq { kind: EqKind, frequency_hz: f32, gain_db: f32, q: f32 },
    /// Turns down what's over the threshold by the ratio.
    Compressor { threshold_db: f32, ratio: f32, attack_ms: f32, release_ms: f32, makeup_db: f32 },
}

/// Where an [`BusEffect::Eq`] acts.
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
pub enum EqKind {
    /// Around the frequency.
    Bell,
    /// Around and below it.
    LowShelf,
    /// Around and above it.
    HighShelf,
}

/// What a room sounds like (M6 U5): kira's Freeverb, which a zone's room names. The room the listener stands in
/// plays it, and a voice reaches it from that room, or from anywhere outdoors when the room is outdoors, at its
/// bus's `room_db`.
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
pub struct Reverb {
    /// What a room names it by.
    pub name: String,
    /// How long it rings, 0 to under 1.
    pub feedback: f32,
    /// How fast its highs die, 0 to 1.
    pub damping: f32,
    /// 0 is mono, 1 full width.
    pub stereo_width: f32,
    /// What it gives back, in decibels.
    #[serde(default)]
    pub volume_db: f32,
}

/// A change to the mix the game asks for. See the [module docs](self).
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
pub struct Snapshot {
    /// What the game asks for it by.
    pub name: String,
    /// Higher wins a low-pass or a pitch over lower, as far as it's in.
    #[serde(default)]
    pub priority: u8,
    /// Seconds for it to go about two thirds of the way in.
    pub attack: f32,
    /// Seconds for it to go about two thirds of the way back out.
    pub release: f32,
    #[serde(default)]
    pub driver: SnapshotDriver,
    /// What it does to each bus it names.
    pub buses: Vec<SnapshotBus>,
}

/// What puts a snapshot in.
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
pub enum SnapshotDriver {
    /// The game asking for it by name: wholly in while it's asked.
    #[default]
    Asked,
    /// The world's clock slowing: in by how far its scale has fallen from 1 toward `floor`, wholly at and below.
    WorldTimeScale { floor: f32 },
}

/// A snapshot's offsets to one bus, at their fullest.
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
pub struct SnapshotBus {
    pub bus: String,
    /// Added to its gain, in decibels.
    #[serde(default)]
    pub gain_db: f32,
    /// A low-pass closed to this cutoff, in hertz.
    #[serde(default)]
    pub low_pass_hz: Option<f32>,
    /// Its voices' pitch, and those of the buses under it.
    #[serde(default)]
    pub pitch: Option<f32>,
    /// Held where it is, tails and all, while the snapshot is asked.
    #[serde(default)]
    pub pause: bool,
}

/// Buses down while another's voices sound.
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
pub struct DuckRule {
    /// The bus whose voices, or those of a bus under it, duck the others.
    pub when: String,
    pub follow: DuckFollow,
    /// Seconds for a duck to go about two thirds of the way down.
    pub attack: f32,
    /// Seconds for it to go about two thirds of the way back up.
    pub release: f32,
    /// The buses it takes down, and how far at the fullest.
    pub duck: Vec<DuckTarget>,
}

/// What a ducking rule follows.
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
pub enum DuckFollow {
    /// The loudest voice's cooked mouth envelope at its playhead ([`MouthCurve`](crate::mouth::MouthCurve)'s
    /// `open`), full from `full_at` up and none at its silence. A voice whose sound has no curve ducks fully
    /// while it plays.
    Envelope { full_at: f32 },
    /// Fully while any voice plays.
    Presence,
}

/// One bus a rule takes down.
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
pub struct DuckTarget {
    pub bus: String,
    /// How far down at the fullest, in decibels: 0 or below.
    pub gain_db: f32,
}

/// Where the sound is heard, which says how loud and how narrow the whole mix should be.
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
pub enum OutputProfile {
    /// Speakers or headphones at home, with a wide range.
    #[default]
    Home,
    /// A phone's speaker or a commute's earbuds: louder and narrower.
    Portable,
}

/// How far a render through the shipped mix may read from its profile's loudness, in LU (M6 U10).
pub const LOUDNESS_TOLERANCE_LU: f32 = 2.0;

impl OutputProfile {
    /// Both, `Home` first.
    pub const ALL: [Self; 2] = [Self::Home, Self::Portable];

    /// The integrated loudness a game plays at through this profile, in LUFS (M6 U10, m6-plan §3's "Loudness"):
    /// what a mix's [`Profile::volume_db`] is set to reach, and what CI meters its renders against, within
    /// [`LOUDNESS_TOLERANCE_LU`].
    #[must_use]
    pub const fn loudness_lufs(self) -> f32 {
        match self {
            Self::Home => -24.0,
            Self::Portable => -18.0,
        }
    }
}

/// The master's volume and effects for one [`OutputProfile`].
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
pub struct Profile {
    pub profile: OutputProfile,
    /// The master's gain: what brings the game to [`OutputProfile::loudness_lufs`] (M6 U10), a fixed gain rather
    /// than a loop that listens.
    #[serde(default)]
    pub volume_db: f32,
    #[serde(default)]
    pub effects: Vec<BusEffect>,
}

/// A mix that can't be read.
#[derive(Debug, thiserror::Error)]
pub enum MixError {
    #[error(transparent)]
    Document(#[from] DocumentProblem),
    /// A value that parsed but can't be mixed, by its path in the document.
    #[error("`{field}`: {problem}")]
    Field { field: String, problem: String },
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

/// Why a snapshot's pitch, pause or low-pass, or a ducking rule's `when`, can't name a bus below a category.
const BELOW_A_CATEGORY: &str = "voices play on the categories, so this changes nothing heard on a bus below one: \
                                name the category under which it lies";

fn invalid(field: impl Into<String>, problem: impl Into<String>) -> MixError {
    MixError::Field { field: field.into(), problem: problem.into() }
}

impl Mix {
    /// The five categories and nothing else, at home: what a mixer plays before it's given a mix.
    #[must_use]
    pub fn plain() -> Self {
        let bus = |bus: Bus| MixBus {
            name: bus.mix_name().to_owned(),
            volume_db: 0.0,
            effects: Vec::new(),
            room_db: None,
        };
        Self {
            buses: Bus::ALL.map(bus).to_vec(),
            reverbs: Vec::new(),
            snapshots: Vec::new(),
            ducking: Vec::new(),
            profiles: vec![Profile { profile: OutputProfile::Home, volume_db: 0.0, effects: Vec::new() }],
            groups: Vec::new(),
        }
    }

    /// The voice group called `name`, by its place in [`Self::groups`].
    #[must_use]
    pub fn group(&self, name: &str) -> Option<usize> {
        self.groups.iter().position(|group| group.name == name)
    }

    /// Reads and checks a mix document.
    pub fn from_document(text: &str) -> Result<Self, MixError> {
        let mix: Self = read_document(text)?;
        mix.check()?;
        Ok(mix)
    }

    /// The bus called `name`, by its place in [`Self::buses`].
    #[must_use]
    pub fn bus(&self, name: &str) -> Option<usize> {
        self.buses.iter().position(|bus| bus.name == name)
    }

    /// The bus `bus`'s parent, by its place: none for a category.
    #[must_use]
    pub fn parent(&self, bus: usize) -> Option<usize> {
        let (parent, _) = self.buses[bus].name.rsplit_once('/')?;
        self.bus(parent)
    }

    /// A category's bus, by its place: the categories are a checked mix's first five buses, in [`Bus::ALL`]'s
    /// order.
    #[must_use]
    pub const fn category(&self, category: Bus) -> usize {
        category.index()
    }

    /// Whether the bus `bus` is `ancestor` or under it, both by place.
    #[must_use]
    pub fn is_under(&self, mut bus: usize, ancestor: usize) -> bool {
        loop {
            if bus == ancestor {
                return true;
            }
            match self.parent(bus) {
                Some(parent) => bus = parent,
                None => return false,
            }
        }
    }

    /// The reverb called `name`, by its place in [`Self::reverbs`].
    #[must_use]
    pub fn reverb(&self, name: &str) -> Option<usize> {
        self.reverbs.iter().position(|reverb| reverb.name == name)
    }

    /// The level the voices of bus `bus` (by place) send to the listener's room at (M6 U5): its category's
    /// `room_db`, since only a category has one; none sends nothing.
    #[must_use]
    pub fn room_db(&self, mut bus: usize) -> Option<f32> {
        loop {
            if let Some(level) = self.buses[bus].room_db {
                return Some(level);
            }
            bus = self.parent(bus)?;
        }
    }

    /// The profile for `profile`, by its place in [`Self::profiles`]: `Home`'s if the mix has none for it.
    #[must_use]
    pub fn profile(&self, profile: OutputProfile) -> usize {
        let home = || self.profiles.iter().position(|each| each.profile == OutputProfile::Home);
        let found = self.profiles.iter().position(|each| each.profile == profile);
        found.or_else(home).expect("a checked mix has a Home profile")
    }

    /// Checks that every value can be mixed, naming the first that can't by its path.
    pub fn check(&self) -> Result<(), MixError> {
        self.check_buses()?;
        self.check_reverbs()?;
        self.check_snapshots()?;
        self.check_ducking()?;
        self.check_profiles()?;
        self.check_groups()
    }

    fn check_groups(&self) -> Result<(), MixError> {
        if self.groups.len() > MAX_GROUPS {
            return Err(invalid("groups", format!("{} groups is more than {MAX_GROUPS}", self.groups.len())));
        }
        for (index, group) in self.groups.iter().enumerate() {
            let field = format!("groups[{index}]");
            let name = &group.name;
            if name.is_empty()
                || name.trim() != name
                || self.groups[..index].iter().any(|earlier| earlier.name == *name)
            {
                return Err(invalid(
                    format!("{field}.name"),
                    format!("{name:?} is empty, padded or named twice"),
                ));
            }
            if let Some(problem) = group.limit.problem() {
                return Err(invalid(format!("{field}.limit.max"), problem));
            }
        }
        Ok(())
    }

    fn check_buses(&self) -> Result<(), MixError> {
        if !(Bus::ALL.len()..=MAX_BUSES).contains(&self.buses.len()) {
            let count = self.buses.len();
            return Err(invalid(
                "buses",
                format!("{count} buses isn't from {} to {MAX_BUSES}", Bus::ALL.len()),
            ));
        }
        for (index, bus) in self.buses.iter().enumerate() {
            let field = format!("buses[{index}]");
            let name = &bus.name;
            if name.is_empty() || name.split('/').any(str::is_empty) {
                return Err(invalid(format!("{field}.name"), format!("{name:?} isn't a path of names")));
            }
            if self.buses[..index].iter().any(|earlier| earlier.name == *name) {
                return Err(invalid(format!("{field}.name"), format!("{name:?} is named twice")));
            }
            match (Bus::ALL.get(index), name.rsplit_once('/')) {
                (Some(category), _) if *name != category.mix_name() => {
                    let roots = Bus::ALL.map(Bus::mix_name).join(", ");
                    let problem = format!("{name:?} is where the categories go, first and in order: {roots}");
                    return Err(invalid(format!("{field}.name"), problem));
                }
                (None, None) => {
                    let problem =
                        format!("{name:?} isn't under a category: a bus below one is named by its path");
                    return Err(invalid(format!("{field}.name"), problem));
                }
                (None, Some((parent, _)))
                    if self.buses[..index].iter().all(|earlier| earlier.name != parent) =>
                {
                    return Err(invalid(
                        format!("{field}.name"),
                        format!("{name:?}'s parent isn't listed before it"),
                    ));
                }
                _ => {}
            }
            check_gain(&format!("{field}.volume_db"), bus.volume_db)?;
            check_effects(&format!("{field}.effects"), &bus.effects)?;
            if let Some(level) = bus.room_db {
                if index >= Bus::ALL.len() {
                    let problem = "voices play on the categories, so only a category sends to the room's reverb: \
                                   give the category under which it lies a `room_db`";
                    return Err(invalid(format!("{field}.room_db"), problem.to_owned()));
                }
                check_gain(&format!("{field}.room_db"), level)?;
            }
        }
        Ok(())
    }

    fn check_reverbs(&self) -> Result<(), MixError> {
        if self.reverbs.len() > MAX_REVERBS {
            let count = self.reverbs.len();
            return Err(invalid("reverbs", format!("{count} reverbs is more than {MAX_REVERBS}")));
        }
        for (index, reverb) in self.reverbs.iter().enumerate() {
            let field = format!("reverbs[{index}]");
            let name = &reverb.name;
            if name.is_empty()
                || name.trim() != name
                || self.reverbs[..index].iter().any(|earlier| earlier.name == *name)
            {
                return Err(invalid(
                    format!("{field}.name"),
                    format!("{name:?} is empty, padded or named twice"),
                ));
            }
            if !(reverb.feedback.is_finite() && (0.0..1.0).contains(&reverb.feedback)) {
                return Err(invalid(
                    format!("{field}.feedback"),
                    format!("{} isn't from 0 to under 1", reverb.feedback),
                ));
            }
            check_share(&format!("{field}.damping"), reverb.damping)?;
            check_share(&format!("{field}.stereo_width"), reverb.stereo_width)?;
            check_gain(&format!("{field}.volume_db"), reverb.volume_db)?;
        }
        Ok(())
    }

    fn check_snapshots(&self) -> Result<(), MixError> {
        if self.snapshots.len() > MAX_SNAPSHOTS {
            let count = self.snapshots.len();
            return Err(invalid("snapshots", format!("{count} snapshots is more than {MAX_SNAPSHOTS}")));
        }
        for (index, snapshot) in self.snapshots.iter().enumerate() {
            let field = format!("snapshots[{index}]");
            let name = &snapshot.name;
            if name.is_empty() || self.snapshots[..index].iter().any(|earlier| earlier.name == *name) {
                return Err(invalid(format!("{field}.name"), format!("{name:?} is empty or named twice")));
            }
            check_seconds(&format!("{field}.attack"), snapshot.attack)?;
            check_seconds(&format!("{field}.release"), snapshot.release)?;
            if let SnapshotDriver::WorldTimeScale { floor } = snapshot.driver
                && !(floor.is_finite() && (0.0..1.0).contains(&floor))
            {
                return Err(invalid(
                    format!("{field}.driver"),
                    format!("a floor of {floor} isn't from 0 to under 1"),
                ));
            }
            if snapshot.buses.is_empty() {
                return Err(invalid(format!("{field}.buses"), "a snapshot changes at least one bus"));
            }
            for (at, change) in snapshot.buses.iter().enumerate() {
                let field = format!("{field}.buses[{at}]");
                if self.bus(&change.bus).is_none() {
                    return Err(invalid(
                        format!("{field}.bus"),
                        format!("{:?} isn't one of the mix's buses", change.bus),
                    ));
                }
                if snapshot.buses[..at].iter().any(|earlier| earlier.bus == change.bus) {
                    return Err(invalid(
                        format!("{field}.bus"),
                        format!("{:?} is changed twice", change.bus),
                    ));
                }
                check_gain(&format!("{field}.gain_db"), change.gain_db)?;
                if let Some(cutoff) = change.low_pass_hz {
                    check_hz(&format!("{field}.low_pass_hz"), cutoff)?;
                }
                if let Some(pitch) = change.pitch
                    && !(pitch.is_finite() && PITCH_RANGE.contains(&pitch))
                {
                    let (low, high) = (PITCH_RANGE.start(), PITCH_RANGE.end());
                    return Err(invalid(
                        format!("{field}.pitch"),
                        format!("{pitch} isn't from {low} to {high}"),
                    ));
                }
                // The stems keep time with the music's clock, which a pause wouldn't stop.
                let music = self.category(Bus::Music);
                if change.pause && self.bus(&change.bus).is_some_and(|bus| self.is_under(bus, music)) {
                    let problem = "the music isn't paused, since its clock goes on: take it down instead";
                    return Err(invalid(format!("{field}.pause"), problem));
                }
                // Voices play on the categories, so a pitch, a pause or a filter on a bus below one changes nothing
                // heard (fixed after M6 U2's review: they checked and did nothing). A gain moves the bus's track,
                // which carries no sound either until a cue can play below its category (U4): accepted, unheard.
                if self.bus(&change.bus).is_some_and(|bus| bus >= Bus::ALL.len()) {
                    let unheard = [
                        ("pitch", change.pitch.is_some()),
                        ("pause", change.pause),
                        ("low_pass_hz", change.low_pass_hz.is_some()),
                    ];
                    if let Some((name, _)) = unheard.into_iter().find(|&(_, set)| set) {
                        return Err(invalid(format!("{field}.{name}"), BELOW_A_CATEGORY));
                    }
                }
            }
        }
        Ok(())
    }

    fn check_ducking(&self) -> Result<(), MixError> {
        if self.ducking.len() > MAX_RULES {
            return Err(invalid("ducking", format!("{} rules is more than {MAX_RULES}", self.ducking.len())));
        }
        for (index, rule) in self.ducking.iter().enumerate() {
            let field = format!("ducking[{index}]");
            match self.bus(&rule.when) {
                None => {
                    return Err(invalid(
                        format!("{field}.when"),
                        format!("{:?} isn't one of the mix's buses", rule.when),
                    ));
                }
                // No voice plays below a category, so a rule waiting for one would never duck.
                Some(bus) if bus >= Bus::ALL.len() => {
                    return Err(invalid(format!("{field}.when"), BELOW_A_CATEGORY));
                }
                Some(_) => {}
            }
            if let DuckFollow::Envelope { full_at } = rule.follow
                && !(full_at.is_finite() && full_at > 0.0 && full_at <= 1.0)
            {
                return Err(invalid(
                    format!("{field}.follow"),
                    format!("full at {full_at} isn't over 0 and up to 1"),
                ));
            }
            check_seconds(&format!("{field}.attack"), rule.attack)?;
            check_seconds(&format!("{field}.release"), rule.release)?;
            if rule.duck.is_empty() {
                return Err(invalid(format!("{field}.duck"), "a rule takes down at least one bus"));
            }
            for (at, target) in rule.duck.iter().enumerate() {
                let field = format!("{field}.duck[{at}]");
                if self.bus(&target.bus).is_none() {
                    return Err(invalid(
                        format!("{field}.bus"),
                        format!("{:?} isn't one of the mix's buses", target.bus),
                    ));
                }
                if !(target.gain_db.is_finite() && (SILENCE_DB..=0.0).contains(&target.gain_db)) {
                    let gain = target.gain_db;
                    return Err(invalid(
                        format!("{field}.gain_db"),
                        format!("{gain} isn't from {SILENCE_DB} to 0"),
                    ));
                }
            }
        }
        Ok(())
    }

    fn check_profiles(&self) -> Result<(), MixError> {
        for (index, profile) in self.profiles.iter().enumerate() {
            let field = format!("profiles[{index}]");
            if self.profiles[..index].iter().any(|earlier| earlier.profile == profile.profile) {
                return Err(invalid(
                    format!("{field}.profile"),
                    format!("{:?} is listed twice", profile.profile),
                ));
            }
            check_gain(&format!("{field}.volume_db"), profile.volume_db)?;
            check_effects(&format!("{field}.effects"), &profile.effects)?;
        }
        if !self.profiles.iter().any(|profile| profile.profile == OutputProfile::Home) {
            return Err(invalid("profiles", "a mix has a Home profile, which any other falls back to"));
        }
        Ok(())
    }

    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked bytes in place, checking the envelope, the archive and every value.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedMix, MixError> {
        let mix = sapphire_serialize::access::<Self>(bytes)?;
        mix.to_mix().check()?;
        Ok(mix)
    }
}

impl ArchivedMix {
    /// The mix as plain data.
    #[must_use]
    pub fn to_mix(&self) -> Mix {
        rkyv::deserialize::<Mix, rkyv::rancor::Error>(self)
            .expect("an archive that passed its check deserializes")
    }
}

fn check_gain(field: &str, gain_db: f32) -> Result<(), MixError> {
    if gain_db.is_finite() && (SILENCE_DB..=MAX_VOLUME_DB).contains(&gain_db) {
        return Ok(());
    }
    Err(invalid(field, format!("{gain_db} dB isn't from {SILENCE_DB} to {MAX_VOLUME_DB}")))
}

fn check_share(field: &str, share: f32) -> Result<(), MixError> {
    if share.is_finite() && (0.0..=1.0).contains(&share) {
        return Ok(());
    }
    Err(invalid(field, format!("{share} isn't from 0 to 1")))
}

fn check_seconds(field: &str, seconds: f32) -> Result<(), MixError> {
    if seconds.is_finite() && (0.0..=MAX_SECONDS).contains(&seconds) {
        return Ok(());
    }
    Err(invalid(field, format!("{seconds} s isn't from 0 to {MAX_SECONDS}")))
}

fn check_hz(field: &str, hz: f32) -> Result<(), MixError> {
    if hz.is_finite() && HEARD_HZ.contains(&hz) {
        return Ok(());
    }
    Err(invalid(field, format!("{hz} Hz isn't from {} to {}", HEARD_HZ.start(), HEARD_HZ.end())))
}

fn check_effects(field: &str, effects: &[BusEffect]) -> Result<(), MixError> {
    if effects.len() > MAX_EFFECTS {
        return Err(invalid(field, format!("{} effects is more than {MAX_EFFECTS}", effects.len())));
    }
    for (index, effect) in effects.iter().enumerate() {
        let field = format!("{field}[{index}]");
        match *effect {
            BusEffect::LowPass { cutoff_hz } | BusEffect::HighPass { cutoff_hz } => {
                check_hz(&format!("{field}.cutoff_hz"), cutoff_hz)?;
            }
            BusEffect::Eq { frequency_hz, gain_db, q, .. } => {
                check_hz(&format!("{field}.frequency_hz"), frequency_hz)?;
                if !(gain_db.is_finite() && (-24.0..=24.0).contains(&gain_db)) {
                    return Err(invalid(
                        format!("{field}.gain_db"),
                        format!("{gain_db} isn't from -24 to 24"),
                    ));
                }
                if !(q.is_finite() && (0.1..=10.0).contains(&q)) {
                    return Err(invalid(format!("{field}.q"), format!("{q} isn't from 0.1 to 10")));
                }
            }
            BusEffect::Compressor { threshold_db, ratio, attack_ms, release_ms, makeup_db } => {
                if !(threshold_db.is_finite() && (SILENCE_DB..=0.0).contains(&threshold_db)) {
                    let threshold = threshold_db;
                    return Err(invalid(
                        format!("{field}.threshold_db"),
                        format!("{threshold} isn't from {SILENCE_DB} to 0"),
                    ));
                }
                if !(ratio.is_finite() && (1.0..=50.0).contains(&ratio)) {
                    return Err(invalid(format!("{field}.ratio"), format!("{ratio} isn't from 1 to 50")));
                }
                for (name, ms) in [("attack_ms", attack_ms), ("release_ms", release_ms)] {
                    if !(ms.is_finite() && ms > 0.0 && ms <= 5_000.0) {
                        return Err(invalid(
                            format!("{field}.{name}"),
                            format!("{ms} ms isn't over 0 and up to 5000"),
                        ));
                    }
                }
                if !(makeup_db.is_finite() && (0.0..=MAX_VOLUME_DB).contains(&makeup_db)) {
                    return Err(invalid(
                        format!("{field}.makeup_db"),
                        format!("{makeup_db} isn't from 0 to {MAX_VOLUME_DB}"),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A good mix document of the current version, with `extra` lines added to its data.
    fn document(buses: &str, extra: &str) -> String {
        format!(
            "(\n    type: \"sapphire::assets::Mix\",\n    version: 3,\n    data: (\n        buses: [{buses}],\n{extra}\n        profiles: [(profile: Home)],\n    ),\n)\n"
        )
    }

    const CATEGORIES: &str =
        r#"(name: "sfx"), (name: "ambience"), (name: "ui"), (name: "music"), (name: "voice")"#;

    fn error_of(buses: &str, extra: &str) -> String {
        Mix::from_document(&document(buses, extra)).expect_err(extra).to_string()
    }

    #[test]
    fn the_plain_mix_is_a_mix_and_cooks_back() {
        let plain = Mix::plain();
        plain.check().unwrap();
        for bus in Bus::ALL {
            assert_eq!(plain.category(bus), bus.index(), "the categories in their own order");
        }
        let cooked = plain.to_cooked().unwrap();
        assert_eq!(Mix::read(&cooked).unwrap().to_mix(), plain);
    }

    #[test]
    fn a_full_mix_reads_and_cooks_back() {
        let buses = format!(
            r#"{CATEGORIES}, (name: "sfx/hits", volume_db: -2.0, effects: [HighPass(cutoff_hz: 80.0), Eq(kind: Bell, frequency_hz: 2000.0, gain_db: 3.0, q: 1.0)]), (name: "sfx/hits/heavy")"#
        );
        let extra = r#"
        reverbs: [(name: "shelter", feedback: 0.8, damping: 0.3, stereo_width: 1.0)],
        snapshots: [
            (name: "slow_motion", priority: 10, attack: 0.03, release: 0.25, driver: WorldTimeScale(floor: 0.2),
             buses: [(bus: "sfx", low_pass_hz: Some(900.0), pitch: Some(0.75))]),
            (name: "pause", attack: 0.05, release: 0.2, buses: [(bus: "sfx", pause: true), (bus: "music", gain_db: -6.0), (bus: "sfx/hits", gain_db: -3.0)]),
        ],
        ducking: [(when: "voice", follow: Envelope(full_at: 0.5), attack: 0.02, release: 0.03, duck: [(bus: "music", gain_db: -10.0), (bus: "sfx/hits/heavy", gain_db: -2.0)])],"#;
        let mix = Mix::from_document(&document(&buses, extra)).unwrap();
        assert_eq!(mix.parent(mix.bus("sfx/hits/heavy").unwrap()), mix.bus("sfx/hits"));
        assert_eq!(mix.parent(mix.bus("sfx/hits").unwrap()), Some(mix.category(Bus::Sfx)));
        assert_eq!(mix.parent(mix.category(Bus::Music)), None);
        assert_eq!(mix.reverb("shelter"), Some(0));
        let heavy = mix.bus("sfx/hits/heavy").unwrap();
        let sfx = mix.category(Bus::Sfx);
        assert_eq!((mix.room_db(heavy), mix.room_db(sfx)), (None, None), "none sends nothing");
        assert_eq!(mix.snapshots[1].priority, 0);
        assert_eq!(mix.snapshots[1].driver, SnapshotDriver::Asked);
        assert_eq!(mix.profile(OutputProfile::Portable), 0, "Home stands in for a profile the mix lacks");
        let cooked = mix.to_cooked().unwrap();
        assert_eq!(Mix::read(&cooked).unwrap().to_mix(), mix);
    }

    #[test]
    fn a_bad_value_names_its_path() {
        let cases = [
            (format!("{CATEGORIES}, (name: \"sfx/hits\", volume_db: 40.0)"), "", "buses[5].volume_db"),
            (format!("{CATEGORIES}, (name: \"sfx/hits\", effects: [LowPass(cutoff_hz: 5.0)])"), "", "buses[5].effects[0].cutoff_hz"),
            (format!("{CATEGORIES}, (name: \"sfx/hits\", room_db: Some(-12.0))"), "", "buses[5].room_db"),
            (r#"(name: "sfx", room_db: Some(30.0)), (name: "ambience"), (name: "ui"), (name: "music"), (name: "voice")"#.to_owned(), "", "buses[0].room_db"),
            (format!("{CATEGORIES}, (name: \"hits\")"), "", "buses[5].name"),
            (format!("{CATEGORIES}, (name: \"sfx/a/b\")"), "", "buses[5].name"),
            (format!("{CATEGORIES}, (name: \"sfx\")"), "", "buses[5].name"),
            (r#"(name: "sfx"), (name: "ambience"), (name: "ui"), (name: "music")"#.to_owned(), "", "buses"),
            (r#"(name: "sfx"), (name: "ui"), (name: "ambience"), (name: "music"), (name: "voice")"#.to_owned(), "", "buses[1].name"),
            (r#"(name: "sfx"), (name: "sfx/hits"), (name: "ambience"), (name: "ui"), (name: "music"), (name: "voice")"#.to_owned(), "", "buses[1].name"),
            (format!("{CATEGORIES}, (name: \"music/stings\")"), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, buses: [(bus: "music/stings", pause: true)])],"#, "snapshots[0].buses[0].pause"),
            // Below a category, where no voice plays (the M6 backfill): what bends, waits or filters voices, and
            // what waits for them.
            (format!("{CATEGORIES}, (name: \"sfx/hits\")"), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, buses: [(bus: "sfx/hits", gain_db: -3.0, pitch: Some(0.8))])],"#, "snapshots[0].buses[0].pitch"),
            (format!("{CATEGORIES}, (name: \"sfx/hits\")"), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, buses: [(bus: "sfx/hits", pause: true)])],"#, "snapshots[0].buses[0].pause"),
            (format!("{CATEGORIES}, (name: \"sfx/hits\")"), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, buses: [(bus: "sfx", pause: true), (bus: "sfx/hits", low_pass_hz: Some(900.0))])],"#, "snapshots[0].buses[1].low_pass_hz"),
            (format!("{CATEGORIES}, (name: \"voice/lines\")"), r#"ducking: [(when: "voice/lines", follow: Presence, attack: 0.1, release: 0.1, duck: [(bus: "music", gain_db: -3.0)])],"#, "ducking[0].when"),
            (CATEGORIES.to_owned(), r#"reverbs: [(name: "a", feedback: 1.0, damping: 0.0, stereo_width: 1.0)],"#, "reverbs[0].feedback"),
            (CATEGORIES.to_owned(), r#"reverbs: [(name: "a", feedback: 0.5, damping: 2.0, stereo_width: 1.0)],"#, "reverbs[0].damping"),
            (CATEGORIES.to_owned(), r#"reverbs: [(name: "a", feedback: 0.5, damping: 0.5, stereo_width: 1.0), (name: "a", feedback: 0.5, damping: 0.5, stereo_width: 1.0)],"#, "reverbs[1].name"),
            (CATEGORIES.to_owned(), r#"snapshots: [(name: "s", attack: -1.0, release: 0.1, buses: [(bus: "sfx")])],"#, "snapshots[0].attack"),
            (CATEGORIES.to_owned(), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, buses: [(bus: "nope")])],"#, "snapshots[0].buses[0].bus"),
            (CATEGORIES.to_owned(), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, buses: [(bus: "sfx", pitch: Some(9.0))])],"#, "snapshots[0].buses[0].pitch"),
            (CATEGORIES.to_owned(), r#"snapshots: [(name: "s", attack: 0.1, release: 0.1, driver: WorldTimeScale(floor: 1.0), buses: [(bus: "sfx")])],"#, "snapshots[0].driver"),
            (CATEGORIES.to_owned(), r#"ducking: [(when: "voice", follow: Presence, attack: 0.1, release: 0.1, duck: [(bus: "music", gain_db: 3.0)])],"#, "ducking[0].duck[0].gain_db"),
            (CATEGORIES.to_owned(), r#"ducking: [(when: "speech", follow: Presence, attack: 0.1, release: 0.1, duck: [(bus: "music", gain_db: -3.0)])],"#, "ducking[0].when"),
            (CATEGORIES.to_owned(), r#"ducking: [(when: "voice", follow: Envelope(full_at: 0.0), attack: 0.1, release: 0.1, duck: [(bus: "music", gain_db: -3.0)])],"#, "ducking[0].follow"),
            (CATEGORIES.to_owned(), r#"groups: [(name: "steps", limit: (max: 0, rule: RefuseNew))],"#, "groups[0].limit.max"),
            (CATEGORIES.to_owned(), r#"groups: [(name: "steps", limit: (max: 2, rule: StopOldest)), (name: "steps", limit: (max: 3, rule: StopOldest))],"#, "groups[1].name"),
            (CATEGORIES.to_owned(), r#"groups: [(name: "", limit: (max: 2, rule: StopOldest))],"#, "groups[0].name"),
        ];
        for (buses, extra, path) in cases {
            let message = error_of(&buses, extra);
            assert!(message.contains(&format!("`{path}`")), "{path}: {message}");
        }
    }

    #[test]
    fn a_mix_without_home_or_with_unknown_fields_is_refused() {
        let text = document(CATEGORIES, "")
            .replace("profiles: [(profile: Home)]", "profiles: [(profile: Portable)]");
        assert!(Mix::from_document(&text).unwrap_err().to_string().contains("`profiles`"));
        let message = error_of(CATEGORIES, "loudness: 3,");
        assert!(message.contains("loudness: 3,"), "the line is quoted: {message}");
    }

    /// M6 U3: version 2's groups read and cook back; a version 1 mix reads as written, with none, and couldn't name
    /// one.
    #[test]
    fn groups_are_version_twos_and_a_version_one_mix_reads_as_it_did() {
        let groups = r#"groups: [(name: "footsteps", limit: (max: 6, rule: StopOldest)), (name: "barks", limit: (max: 2, rule: StopLowestPriority))],"#;
        let mix = Mix::from_document(&document(CATEGORIES, groups)).unwrap();
        assert_eq!((mix.group("barks"), mix.group("roars")), (Some(1), None));
        assert_eq!(mix.groups[0].limit, Limit { max: 6, rule: crate::cue::LimitRule::StopOldest });
        let cooked = mix.to_cooked().unwrap();
        assert_eq!(Mix::read(&cooked).unwrap().to_mix(), mix);

        let first = |extra: &str| {
            Mix::from_document(&document(CATEGORIES, extra).replace("version: 3", "version: 1"))
        };
        assert_eq!(first("").unwrap(), Mix::from_document(&document(CATEGORIES, "")).unwrap());
        assert!(first("").unwrap().groups.is_empty());
        assert!(first(groups).unwrap_err().to_string().contains("groups"));
    }

    /// M6 U5: an older mix reads as it did if it has no sends, and is refused, naming them, if it has any: reverbs
    /// are rooms' now.
    #[test]
    fn an_older_mix_reads_without_sends_and_is_refused_with_them() {
        let now = Mix::from_document(&document(CATEGORIES, "")).unwrap();
        let sending =
            format!("{CATEGORIES}, (name: \"sfx/hits\", sends: [(to: \"room\", volume_db: -12.0)])");
        let reverb = r#"sends: [(name: "room", feedback: 0.8, damping: 0.3, stereo_width: 1.0)],"#;
        for version in ["version: 1", "version: 2"] {
            let older = |buses: &str, extra: &str| {
                Mix::from_document(&document(buses, extra).replace("version: 3", version))
            };
            assert_eq!(older(CATEGORIES, "sends: [],").unwrap(), now, "{version}");
            for (buses, extra) in [(sending.as_str(), reverb), (CATEGORIES, reverb)] {
                let message = older(buses, extra).unwrap_err().to_string();
                assert!(message.contains("`sends` are gone since version 3"), "{version}: {message}");
            }
        }
    }
}
