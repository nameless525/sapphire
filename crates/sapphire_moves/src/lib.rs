//! Moves as data (spec §5.0, §5.22, `docs/m2-plan.md` §3): what the simulation reads of a move
//! instead of a pose, and the documents a person writes it in.
//!
//! A move's root motion and its hit volumes' paths are sampled once at cook time into per-tick
//! tables, and its frame data is authored in ticks, so the fixed step reads plain numbers and the
//! animation graph stays presentation only.
//!
//! Owns:
//!
//! - [`MoveSetDocument`], the RON a person or the timeline editor writes, checked with messages
//!   naming the move and track ([`Issue`]);
//! - [`MoveSet`], what it compiles to and cooks as: intents, what starts what, a [`FighterDef`], a
//!   [`BrainDef`] and each [`Move`]'s tracks and tables, among them what its hits do ([`Attack`]) and
//!   what a dodge through one does ([`PerfectDodgeWindow`]);
//! - [`BakedTables`], its cooked form and how to read it at any sub-tick;
//! - how a track meets a clock ([`Stretch`], [`Span::live`], [`Span::sweep`]), as pure functions.
//!
//! Plain data with no renderer, world or ECS behind it, so the cooker and a replay job can both link
//! it.
//!
//! Doesn't own: running moves or applying hits (`sapphire_gameplay`), reading a clip and baking these
//! tables from it (the pipeline's `moves` module, M2 N1), or testing hit volumes against anything (M2
//! D4).
//!
//! # The document
//!
//! A `.moves.ron` file. Everything is in ticks of 60 Hz and by name:
//!
//! ```
//! use sapphire_moves::MoveSetDocument;
//!
//! let document = MoveSetDocument::parse(r#"(
//!     type: "sapphire::moves::MoveSetDocument",
//!     version: 1,
//!     data: (
//!         intents: [
//!             // Pressed by a button, usable for 12 ticks. Higher priority wins a tie.
//!             (name: "light", button: Light, buffer: 12, priority: 1),
//!             // Pressed by a brain, say: it needs no button.
//!             (name: "lunge", buffer: 8),
//!         ],
//!         // What starts a move when no move is running.
//!         neutral: [(intent: "light", move: "jab")],
//!         // What makes its actors something a hit can land on. Left out, nothing touches them.
//!         fighter: (
//!             health: 100,
//!             // A hit taking the last of this plays `poise_break` and fills it again.
//!             poise: 40,
//!             // Full again this long after the last hit that took some.
//!             poise_refill: 240,
//!             hurtboxes: [(name: "body", offset: (0.0, 0.9, 0.0), shape: Capsule(radius: 0.35, length: 1.0))],
//!             // The moves reactions play, by a hit's weight: light, medium and heavy.
//!             reactions: (light: "jab", poise_break: "jab", defeat: "jab"),
//!         ),
//!         moves: [(
//!             name: "jab",
//!             // The move lasts their sum: 20 ticks.
//!             startup: 4,
//!             active: 2,
//!             recovery: 14,
//!             // Hand-keyed tables. "From a clip" below is the other way.
//!             motion: (
//!                 root: [(at: 6, offset: (0.0, 0.0, 0.5))],
//!                 paths: [(name: "fist", keys: [(at: 0, offset: (0.2, 1.2, 0.5))])],
//!             ),
//!             // Every window is `from` up to but not including `to`.
//!             hitboxes: [(
//!                 name: "jab_hit",
//!                 from: 4,
//!                 to: 6,
//!                 path: "fist",
//!                 shape: Sphere(radius: 0.3),
//!                 // All optional. Left out, a hit is an event and nothing more.
//!                 attack: (
//!                     damage: 8,
//!                     poise: 6,
//!                     weight: Light,
//!                     // Both owners' clocks freeze this long.
//!                     hitstop: 4,
//!                     knockback: (distance: 0.3, ticks: 8),
//!                 ),
//!             )],
//!             invulnerable: [(from: 0, to: 2)],
//!             // A live swing touching `volume` in this window fires a perfect dodge, once a playing. It
//!             // must lie inside `invulnerable`, and what it does is optional: slowing the world for a
//!             // number of real steps, with this actor running at full speed, and opening a counter
//!             // window for some ticks of its own clock.
//!             perfect_dodge: [(
//!                 from: 0,
//!                 to: 2,
//!                 volume: (offset: (0.0, 0.9, 0.0), shape: Sphere(radius: 0.5)),
//!                 slow_motion: (per_mille: 200, steps: 30),
//!                 counter: (ticks: 45, allow: [(intent: "light", move: "jab")]),
//!             )],
//!             // Below this much poise, a hit does its damage and no reaction plays.
//!             armour: [(from: 0, to: 4, threshold: 10)],
//!             cancels: [(from: 10, to: 18, allow: [
//!                 (intent: "light", move: "jab"),
//!                 (intent: "lunge", move: "jab", when: Instant),
//!             ])],
//!             warps: [(from: 0, to: 6, max_distance: 1.0, max_turn: 0.5, keep: 0.8)],
//!             cues: [(name: "jab_swing", at: 4)],
//!         )],
//!     ),
//! )"#)
//! .unwrap();
//!
//! let set = document.compile().unwrap();
//! let cooked = set.to_cooked().unwrap();
//! let set = sapphire_moves::MoveSet::from_cooked(&cooked).unwrap();
//! assert_eq!(set.moves()[0].ticks(), 20);
//! ```
//!
//! # From a clip
//!
//! A move plays a clip by naming it in `motion`, in place of keys, and says which of its joints the
//! hit paths follow. A cook bakes the tables, and the simulation reads them as it does hand keys: it
//! never sees the clip.
//!
//! ```ron
//! (
//!     name: "strike",
//!     // These add up to the clip's length. Anything else is an error, saying both.
//!     startup: 14,
//!     active: 6,
//!     recovery: 40,
//!     motion: (
//!         clip: (
//!             // The sidecar ID of a key-pose document: `strike.keypose.ron`.
//!             source: "15af86bb-67b3-4208-bc54-1618aabf450b",
//!             paths: [
//!                 // A humanoid bone, with the blade's tip 90 cm along its +Z.
//!                 (name: "blade", follows: Bone("RightHand"), offset: (0.0, 0.0, 0.9)),
//!             ],
//!         ),
//!     ),
//!     hitboxes: [(name: "strike_hit", from: 14, to: 20, path: "blade", shape: Sphere(radius: 0.3))],
//! )
//! ```
//!
//! - **Which clip:** `source` is a sidecar ID. A key-pose document is one clip per rig it cooks for:
//!   its sidecar's `rig` unless `rig` here names one of its `also`. An animation source also takes
//!   `name`, the clip's name in the file, and `rig` if the clip is one its sidecar retargets:
//!   `(source: "<KayKit file>", name: "Dodge_Forward", rig: "<the character>")`. The sidecar has to
//!   cook that clip: this names it, and doesn't ask for it.
//! - **Root motion:** the clip's own root track, or `root: InPlace` for none. Root keys in `motion`
//!   replace it, for an in-place clip that lunges by hand. To lift a bone a clip moves, lift it in the
//!   clip's sidecar (`root_motion`), so the pose on screen loses what the table gains.
//! - **Paths:** `Bone("RightHand")` is a humanoid bone, as a key-pose document spells it, so one
//!   document fits any rig with a map; `Joint("handslot.r")` is a joint by its name. `offset` (metres)
//!   and `rotation` make either a socket, along the joint's own axes and unscaled. Hand-keyed `paths`
//!   sit beside them.
//! - **One tick of a move is one tick of its clip.** Nothing retimes a clip: presentation plays it at
//!   the move's elapsed ticks, so a hit lands where the blade is drawn only if both read the same
//!   tick. A change of frame data that changes a move's length changes its clip too, and until they
//!   agree the cook says so. Windows inside the move move freely, as they read the same table.
//! - **What a path can't follow:** a joint a rig's constraint drives, or one below such a joint. The
//!   table has no constraints in it and the pose on screen does, so they'd disagree.
//!
//! # One document, one set of bytes
//!
//! A document cooks to the same bytes on every architecture, because one replay file holds one hash
//! of the move data for all of them. So [`MoveSetDocument::compile`] works in scalars and
//! `sapphire_math::det`, never through glam's SIMD types, whose last bit differs between NEON and
//! SSE2. This crate's `clippy.toml` bans the methods that differ, and
//! `sapphire_gameplay`'s `the_test_contents_hash_is_the_same_on_every_architecture` pins the result.
//! So does the cook of a move from a clip, by the same rule, from the clip's source on: see the
//! pipeline's `moves` module.
//!
//! # How a track meets a clock
//!
//! Frame data is in ticks, and an owner's clock runs in whole sub-ticks at any speed, frozen or many
//! ticks a step. Each step's reading of a move is one [`Stretch`] of the move's time, so:
//!
//! - **An instant** (a cue) lies in exactly one stretch, however the clock moved.
//! - **A window** counts on every step whose reading lies in it, and on the step that passed it if
//!   none did ([`Span::live`]). A frozen clock stays in the windows it's in and reaches no new ones.
//! - **A hit volume's arc** runs from its window's first tick to its last, and steps cover it end to
//!   end at any speed ([`Span::sweep`]).
//!
//! At a steady 1×, then, a window `5..8` counts on exactly the steps that read ticks 5, 6 and 7.
//!
//! # What the tables mean
//!
//! Samples are one per fixed tick (60 Hz, `sapphire_time::TICK_HZ`), sample 0 at the move's first
//! tick and sample `n` at `n` ticks of the owner's local clock after it. A move lasting `n` ticks
//! has `n + 1` samples: its first and last poses are both in the table.
//!
//! - **Root motion is cumulative, in the root's space at the move's first tick.** Sample 0 is
//!   always zero. Yaw is unwrapped radians about +Y, so a sample may read past a half turn and a
//!   blend never takes the long way. Deltas between readings telescope, which is why a clock that
//!   stops or slows can't lose or gain any distance.
//! - **A path is a transform per tick, in the root's space at that same tick.** A hit volume rides
//!   the character's current root, so motion warp carries it along. A path with a [`Name`] is what
//!   a move's hit volume names: a blade's tip, a fist.
//!
//! Readings take a [`SimDuration`], whole sub-ticks on the owner's clock. Between two samples they
//! blend linearly (rotations by [`det::quat_nlerp`]), and on a whole tick they return the sample's
//! exact bits, so a clock under hitstop or any time scale reads one number per sub-tick on every
//! target. Past the end they hold the last sample.
//!
//! # Example
//!
//! ```
//! use sapphire_core::Name;
//! use sapphire_moves::{BakedPath, BakedTables, PathSample, RootSample};
//! use sapphire_time::SimDuration;
//!
//! let root = vec![
//!     RootSample::ZERO,
//!     RootSample { translation: [0.0, 0.0, 0.5], yaw: 0.0 },
//!     RootSample { translation: [0.0, 0.0, 1.5], yaw: 0.25 },
//! ];
//! let blade = Name::new("blade");
//! let path = BakedPath::new(blade, vec![PathSample::IDENTITY; 3]);
//! let tables = BakedTables::new(root, vec![path]).unwrap();
//!
//! // A cook writes it, and the game reads it back.
//! let cooked = tables.to_cooked().unwrap();
//! let tables = BakedTables::from_cooked(&cooked).unwrap();
//!
//! assert_eq!(tables.ticks(), 2);
//! // Half a tick past the first tick, on the owner's clock.
//! let reading = tables.root_at(SimDuration::from_subticks(1500));
//! assert_eq!(reading.translation, [0.0, 0.0, 1.0]);
//! assert_eq!(reading.yaw, 0.125);
//! assert!(tables.path(blade).is_some());
//! ```

mod brain;
mod document;
mod set;

pub use brain::{
    Against, BrainAttack, BrainDef, BrainPace, BrainRise, MAX_BRAIN_ATTACKS, Reach, Recent, Spacing,
    TargetDoing, TickRange,
};
pub use document::{
    AllowDocument, ArmourDocument, AttackDocument, BrainAttackDocument, BrainDocument, BrainRiseDocument,
    CancelDocument, ClipMotionDocument, ClipPathDocument, CounterDocument, CueDocument, DodgeVolumeDocument,
    FighterDocument, FighterHurtboxDocument, Follows, HitboxDocument, HurtboxDocument, IntentDocument, Issue,
    MotionDocument, MoveDocument, MoveSetDocument, PathDocument, PathKey, PerfectDodgeDocument,
    ReactionsDocument, RootKey, RootSource, SlowMotionDocument, SpanDocument, WarpDocument,
};
pub use set::{
    Allow, ArchivedMoveSet, ArmourWindow, Attack, Button, CancelMode, CancelWindow, CounterWindow, Cue,
    DodgeVolume, FighterDef, FighterHurtbox, HIT_LANDED, Hitbox, HurtboxChange, HurtboxState, IntentDef,
    Knockback, MAX_BUFFER_TICKS, MAX_HITSTOP_TICKS, MAX_HURTBOXES, MAX_INTENTS, MAX_MOVE_TICKS,
    MAX_SLOW_MOTION_STEPS, MOVE_SET_EXTENSION, Move, MoveSet, MoveSetError, PerfectDodgeWindow, Phase,
    Reactions, Shape, Span, Stretch, WarpWindow, Weight, WorldSlowMotion,
};

use bevy_reflect::TypePath;
use sapphire_core::Name;
use sapphire_math::{Quat, Vec3, det};
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{self, AlignedVec, CookedError};
use sapphire_time::{SUBTICKS_PER_TICK, SimDuration};

/// Conventional file extension for cooked tables.
pub const TABLES_EXTENSION: &str = "smtab";

/// How far a path's rotation may be from unit length, in squared length: a cook's rounding stays
/// far inside it, and a bug or a corrupt file doesn't.
const UNIT_TOLERANCE: f32 = 2e-3;

/// One tick of root motion. See the [crate docs](crate).
#[derive(Clone, Copy, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RootSample {
    /// Metres from the root at the move's first tick, in that root's space.
    pub translation: [f32; 3],
    /// Radians about +Y since the move's first tick, unwrapped.
    pub yaw: f32,
}

impl RootSample {
    /// No motion: every table's sample 0.
    pub const ZERO: Self = Self { translation: [0.0; 3], yaw: 0.0 };

    /// [`Self::translation`] as a vector.
    #[must_use]
    pub fn offset(self) -> Vec3 {
        Vec3::from_array(self.translation)
    }

    fn is_finite(self) -> bool {
        self.translation.iter().all(|component| component.is_finite()) && self.yaw.is_finite()
    }
}

/// One tick of a path. See the [crate docs](crate).
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PathSample {
    /// Metres from the root at this tick, in that root's space.
    pub translation: [f32; 3],
    /// Unit rotation from the root at this tick, as x, y, z, w.
    pub rotation: [f32; 4],
}

impl PathSample {
    /// At the root, turned as the root is.
    pub const IDENTITY: Self = Self { translation: [0.0; 3], rotation: [0.0, 0.0, 0.0, 1.0] };

    /// [`Self::translation`] as a vector.
    #[must_use]
    pub fn offset(self) -> Vec3 {
        Vec3::from_array(self.translation)
    }

    /// [`Self::rotation`] as a quaternion.
    #[must_use]
    pub fn orientation(self) -> Quat {
        Quat::from_array(self.rotation)
    }

    fn is_finite(self) -> bool {
        self.translation.iter().chain(&self.rotation).all(|component| component.is_finite())
    }

    fn is_unit(self) -> bool {
        let length_squared: f32 = self.rotation.iter().map(|component| component * component).sum();
        (length_squared - 1.0).abs() <= UNIT_TOLERANCE
    }
}

/// A named path a hit volume can ride. See the [crate docs](crate).
#[derive(Clone, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BakedPath {
    /// The path's [`Name`], as its hash: runtime comparisons never need the text (spec §5.3).
    name: u64,
    samples: Vec<PathSample>,
}

impl BakedPath {
    /// A path with one sample per tick. [`BakedTables::new`] checks it against its table.
    #[must_use]
    pub fn new(name: Name, samples: Vec<PathSample>) -> Self {
        Self { name: name.as_u64(), samples }
    }

    /// The path's name.
    #[must_use]
    pub fn name(&self) -> Name {
        Name::from_u64(self.name)
    }

    /// One sample per tick.
    #[must_use]
    pub fn samples(&self) -> &[PathSample] {
        &self.samples
    }

    /// The path's transform `elapsed` into the move, between ticks by a blend; see the
    /// [crate docs](crate).
    #[must_use]
    pub fn at(&self, elapsed: SimDuration) -> PathSample {
        let (first, second, blend) = locate(self.samples.len(), elapsed);
        let (Some(&a), Some(&b)) = (self.samples.get(first), self.samples.get(second)) else {
            return PathSample::IDENTITY;
        };
        if blend == 0.0 {
            return a;
        }
        PathSample {
            translation: lerp3(a.translation, b.translation, blend),
            rotation: det::quat_nlerp(a.orientation(), b.orientation(), blend).to_array(),
        }
    }
}

/// One move's baked tables: root motion and hit-volume paths. See the [crate docs](crate).
///
/// Every value holds the checks [`Self::new`] makes, whichever way it was made.
#[derive(Clone, Debug, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[type_path = "sapphire::moves"]
pub struct BakedTables {
    /// One sample per tick, and at least one.
    root: Vec<RootSample>,
    /// Sorted by name's hash, each name once, each with as many samples as `root`.
    paths: Vec<BakedPath>,
}

impl Versioned for BakedTables {
    /// 1: M2 D1.
    const VERSION: u32 = 1;
}

/// Tables that can't be built, read or written.
#[derive(Debug, thiserror::Error)]
pub enum TablesError {
    #[error("a table needs at least one root sample")]
    Empty,
    #[error("root sample 0 must be zero, since root motion counts from the move's first tick")]
    RootNotAtOrigin,
    #[error("root sample {tick} isn't finite")]
    RootNotFinite { tick: usize },
    #[error("path {name:?} has {samples} samples, but the table has {ticks} root samples")]
    PathLength { name: Name, samples: usize, ticks: usize },
    #[error("path {name:?} appears twice, or out of order in cooked data")]
    PathOrder { name: Name },
    #[error("path {name:?} isn't finite at tick {tick}")]
    PathNotFinite { name: Name, tick: usize },
    #[error("path {name:?} doesn't have a unit rotation at tick {tick}")]
    PathNotUnit { name: Name, tick: usize },
    #[error(transparent)]
    Cooked(#[from] CookedError),
}

impl BakedTables {
    /// Tables from one root sample per tick and any named paths, each with as many samples. Paths
    /// may come in any order. See the [crate docs](crate) for what a sample means.
    pub fn new(root: Vec<RootSample>, mut paths: Vec<BakedPath>) -> Result<Self, TablesError> {
        paths.sort_by_key(|path| path.name);
        let tables = Self { root, paths };
        tables.check()?;
        Ok(tables)
    }

    /// Reads cooked tables, whose bytes must start 16-byte aligned, after validating the archive
    /// and every check [`Self::new`] makes. Copies them out, once, at load: reading a table has no
    /// header to check and no `unsafe` that way, and never allocates.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, TablesError> {
        let archived = cooked::access::<Self>(bytes)?;
        let tables = rkyv::deserialize::<Self, rkyv::rancor::Error>(archived).map_err(CookedError::from)?;
        tables.check()?;
        Ok(tables)
    }

    /// Writes cooked tables.
    pub fn to_cooked(&self) -> Result<AlignedVec, TablesError> {
        Ok(cooked::to_cooked(self)?)
    }

    pub(crate) fn check(&self) -> Result<(), TablesError> {
        let first = self.root.first().ok_or(TablesError::Empty)?;
        if first.translation != RootSample::ZERO.translation || first.yaw != RootSample::ZERO.yaw {
            return Err(TablesError::RootNotAtOrigin);
        }
        if let Some(tick) = self.root.iter().position(|sample| !sample.is_finite()) {
            return Err(TablesError::RootNotFinite { tick });
        }
        let mut previous = None;
        for path in &self.paths {
            let name = path.name();
            if previous.is_some_and(|hash| hash >= path.name) {
                return Err(TablesError::PathOrder { name });
            }
            previous = Some(path.name);
            if path.samples.len() != self.root.len() {
                let (samples, ticks) = (path.samples.len(), self.root.len());
                return Err(TablesError::PathLength { name, samples, ticks });
            }
            if let Some(tick) = path.samples.iter().position(|sample| !sample.is_finite()) {
                return Err(TablesError::PathNotFinite { name, tick });
            }
            if let Some(tick) = path.samples.iter().position(|sample| !sample.is_unit()) {
                return Err(TablesError::PathNotUnit { name, tick });
            }
        }
        Ok(())
    }

    /// Ticks from the first sample to the last: one fewer than there are samples.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        u32::try_from(self.root.len() - 1).unwrap_or(u32::MAX)
    }

    /// The owner's time from the first sample to the last.
    #[must_use]
    pub fn duration(&self) -> SimDuration {
        SimDuration::from_ticks(u64::from(self.ticks()))
    }

    /// One root sample per tick.
    #[must_use]
    pub fn root_samples(&self) -> &[RootSample] {
        &self.root
    }

    /// The root motion `elapsed` into the move, between ticks by a blend; see the
    /// [crate docs](crate).
    #[must_use]
    pub fn root_at(&self, elapsed: SimDuration) -> RootSample {
        let (first, second, blend) = locate(self.root.len(), elapsed);
        let (a, b) = (self.root[first], self.root[second]);
        if blend == 0.0 {
            return a;
        }
        RootSample {
            translation: lerp3(a.translation, b.translation, blend),
            yaw: a.yaw + (b.yaw - a.yaw) * blend,
        }
    }

    /// The paths, sorted by the hash of their names.
    #[must_use]
    pub fn paths(&self) -> &[BakedPath] {
        &self.paths
    }

    /// The path called `name`, by binary search, which allocates nothing.
    #[must_use]
    pub fn path(&self, name: Name) -> Option<&BakedPath> {
        let index = self.paths.binary_search_by_key(&name.as_u64(), |path| path.name).ok()?;
        self.paths.get(index)
    }
}

/// Where `elapsed` falls among `count` samples: the sample at or before it, the next one, and how
/// far between them. Past the last sample, both are the last, with no blend.
#[inline]
fn locate(count: usize, elapsed: SimDuration) -> (usize, usize, f32) {
    let last = count.saturating_sub(1);
    let tick = usize::try_from(elapsed.whole_ticks()).unwrap_or(usize::MAX);
    if tick >= last {
        return (last, last, 0.0);
    }
    let into_tick = elapsed.as_subticks() % u64::from(SUBTICKS_PER_TICK);
    (tick, tick + 1, into_tick as f32 / SUBTICKS_PER_TICK as f32)
}

#[inline]
fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLADE: Name = Name::new("blade");
    const FIST: Name = Name::new("fist");

    fn root(ticks: u32) -> Vec<RootSample> {
        (0..=ticks)
            .map(|tick| {
                let t = tick as f32;
                RootSample { translation: [t * 0.25, 0.0, t * t * 0.01], yaw: t * 0.05 }
            })
            .collect()
    }

    fn path(name: Name, ticks: u32) -> BakedPath {
        let samples = (0..=ticks)
            .map(|tick| {
                let t = tick as f32 * 0.1;
                PathSample {
                    translation: [t, 1.0 + t, -t],
                    rotation: det::quat_from_rotation_y(t).to_array(),
                }
            })
            .collect();
        BakedPath::new(name, samples)
    }

    fn sub(subticks: u64) -> SimDuration {
        SimDuration::from_subticks(subticks)
    }

    #[test]
    fn whole_ticks_read_a_samples_exact_bits() {
        let tables = BakedTables::new(root(30), vec![path(BLADE, 30)]).unwrap();
        for tick in 0..=30_u32 {
            let elapsed = SimDuration::from_ticks(u64::from(tick));
            let (reading, sample) = (tables.root_at(elapsed), tables.root_samples()[tick as usize]);
            assert_eq!(reading.translation.map(f32::to_bits), sample.translation.map(f32::to_bits));
            assert_eq!(reading.yaw.to_bits(), sample.yaw.to_bits());
            let blade = tables.path(BLADE).unwrap();
            assert_eq!(blade.at(elapsed), blade.samples()[tick as usize]);
        }
    }

    #[test]
    fn sub_ticks_blend_between_samples() {
        let tables = BakedTables::new(root(4), vec![]).unwrap();
        let (a, b) = (tables.root_samples()[2], tables.root_samples()[3]);
        // 2750 sub-ticks: three quarters of the way from sample 2 to sample 3.
        let reading = tables.root_at(sub(2750));
        assert_eq!(reading.translation[0], a.translation[0] + (b.translation[0] - a.translation[0]) * 0.75);
        assert_eq!(reading.yaw, a.yaw + (b.yaw - a.yaw) * 0.75);
        // One sub-tick is a thousandth of a tick, computed from whole numbers.
        let one = tables.root_at(sub(2001)).translation[0];
        assert_eq!(one, a.translation[0] + (b.translation[0] - a.translation[0]) * (1.0 / 1000.0));
    }

    #[test]
    fn path_rotations_blend_to_unit_length() {
        let tables = BakedTables::new(root(10), vec![path(BLADE, 10)]).unwrap();
        let blade = tables.path(BLADE).unwrap();
        let halfway = blade.at(sub(3500));
        let (a, b) = (blade.samples()[3], blade.samples()[4]);
        assert!(halfway.offset().abs_diff_eq((a.offset() + b.offset()) * 0.5, 1e-6));
        assert!(halfway.orientation().is_normalized());
        let expected = det::quat_from_rotation_y(0.35);
        assert!(halfway.orientation().abs_diff_eq(expected, 1e-5), "{:?}", halfway.orientation());
    }

    #[test]
    fn readings_hold_the_last_sample_past_the_end() {
        let tables = BakedTables::new(root(3), vec![path(BLADE, 3)]).unwrap();
        let last = *tables.root_samples().last().unwrap();
        assert_eq!(tables.ticks(), 3);
        assert_eq!(tables.duration(), SimDuration::from_ticks(3));
        for elapsed in [3000, 3001, 9999, u64::MAX] {
            assert_eq!(tables.root_at(sub(elapsed)), last, "{elapsed}");
            let blade = tables.path(BLADE).unwrap();
            assert_eq!(blade.at(sub(elapsed)), *blade.samples().last().unwrap(), "{elapsed}");
        }
        // A single sample is a move with no length.
        let still = BakedTables::new(vec![RootSample::ZERO], vec![]).unwrap();
        assert_eq!(still.ticks(), 0);
        assert_eq!(still.root_at(sub(500)), RootSample::ZERO);
    }

    #[test]
    fn paths_are_found_by_name_in_any_order() {
        let tables = BakedTables::new(root(2), vec![path(FIST, 2), path(BLADE, 2)]).unwrap();
        assert_eq!(tables.path(FIST).unwrap().name(), FIST);
        assert_eq!(tables.path(BLADE).unwrap().name(), BLADE);
        assert!(tables.path(Name::new("shin")).is_none());
        let sorted = BakedTables::new(root(2), vec![path(BLADE, 2), path(FIST, 2)]).unwrap();
        assert_eq!(tables, sorted, "the same paths are the same tables");
        assert_eq!(*tables.to_cooked().unwrap(), *sorted.to_cooked().unwrap());
    }

    #[test]
    fn bad_tables_are_refused() {
        assert!(matches!(BakedTables::new(vec![], vec![]), Err(TablesError::Empty)));

        let moved = RootSample { translation: [0.1, 0.0, 0.0], yaw: 0.0 };
        assert!(matches!(BakedTables::new(vec![moved], vec![]), Err(TablesError::RootNotAtOrigin)));

        let mut nan = root(3);
        nan[2].yaw = f32::NAN;
        assert!(matches!(BakedTables::new(nan, vec![]), Err(TablesError::RootNotFinite { tick: 2 })));

        let short = path(BLADE, 2);
        assert!(matches!(
            BakedTables::new(root(3), vec![short]),
            Err(TablesError::PathLength { samples: 3, ticks: 4, .. })
        ));

        let twice = BakedTables::new(root(2), vec![path(BLADE, 2), path(BLADE, 2)]);
        assert!(matches!(twice, Err(TablesError::PathOrder { .. })));

        let mut infinite = path(BLADE, 2);
        infinite.samples[1].translation[1] = f32::INFINITY;
        let refused = BakedTables::new(root(2), vec![infinite]);
        assert!(matches!(refused, Err(TablesError::PathNotFinite { tick: 1, .. })));

        let mut long = path(BLADE, 2);
        long.samples[2].rotation = [0.0, 0.0, 0.0, 2.0];
        let refused = BakedTables::new(root(2), vec![long]);
        assert!(matches!(refused, Err(TablesError::PathNotUnit { tick: 2, .. })));
    }

    #[test]
    fn cooked_tables_round_trip_and_read_alike() {
        let tables = BakedTables::new(root(12), vec![path(BLADE, 12), path(FIST, 12)]).unwrap();
        let cooked = tables.to_cooked().unwrap();
        let back = BakedTables::from_cooked(&cooked).unwrap();
        assert_eq!(back, tables);
        assert_eq!(back.root_at(sub(4321)), tables.root_at(sub(4321)));
        assert_eq!(*back.to_cooked().unwrap(), *cooked, "a cook is byte-stable");
    }

    #[test]
    fn stale_and_damaged_cooked_data_is_an_error() {
        let tables = BakedTables::new(root(2), vec![path(BLADE, 2)]).unwrap();
        let cooked = tables.to_cooked().unwrap();

        // The schema version sits at byte 16 of the header.
        let mut stale = cooked.clone();
        stale[16] += 1;
        assert!(matches!(
            BakedTables::from_cooked(&stale),
            Err(TablesError::Cooked(CookedError::Stale { found: 2, current: 1 }))
        ));

        let cut = &cooked[..cooked.len() - 8];
        assert!(matches!(BakedTables::from_cooked(cut), Err(TablesError::Cooked(_))));

        // Bytes that pass the archive's check but break a table's own: a moved first sample. Sample
        // 0's first float is the archive's first bytes, a 32-byte header in.
        let mut moved = cooked.clone();
        moved[32..36].copy_from_slice(&1.0_f32.to_le_bytes());
        assert!(matches!(BakedTables::from_cooked(&moved), Err(TablesError::RootNotAtOrigin)));
    }
}
