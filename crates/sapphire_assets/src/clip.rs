//! Cooked animation clips (`.sclip`, M2 N1): one motion for one skeleton, read in place and
//! sampled at any time without state or allocation.
//!
//! The cooker (`sapphire_pipeline`) writes a [`Clip`] from a glTF animation or a key-pose
//! document; `sapphire_anim` samples it. The element encodings and what a track means at a time
//! live here, so cooker and runtime can't drift apart: the cook measures its key reduction through
//! [`ArchivedClip::rotation`] and its neighbors, the very functions a sampler calls.
//!
//! # Time
//!
//! A clip is a whole number of ticks long ([`Clip::ticks`]) at its [`Clip::rate`]: 60 a second
//! unless its source asks otherwise, the simulation's own tick, so a move's frame data and its
//! clip count alike. Every key sits on a tick, and values between keys blend linearly, so one
//! interpolation path serves every source: a cook samples a source's step and cubic segments, and
//! a document's eases, onto the ticks. A sampler asks for a *position*, a tick with a fraction
//! ([`ArchivedClip::position`] turns seconds into one).
//!
//! # Layout
//!
//! - **Tracks are sparse over joints.** A sampler starts from the skeleton's rest pose and
//!   overlays tracks, and a channel that stays at rest has none. Most joints of most clips move
//!   only in rotation, so translation and scale tracks are few.
//! - **Keys are sparse over ticks, per track.** The cook drops the keys a track can do without,
//!   and each track keeps its own. Each channel holds one array of ticks and one of keys, parallel,
//!   with its tracks' keys contiguous and in joint order. So what a sampler touches is near what
//!   it touched last, and a binary search reads only ticks. A track of one key holds still.
//! - **Finding keys is a binary search per track.** No per-instance cursor, as ozz-animation's
//!   sampling context is: a cursor costs memory per character and clip, must be reset on every
//!   jump, and every cancel, hitstop and blend is a jump. Five comparisons on a cache line of u16s
//!   for a track of 32 keys, per track, is small against N2's budget (spec §3.2).
//!
//! # Key encodings
//!
//! | Channel | Bytes per key | Encoding | Error |
//! |---|---|---|---|
//! | rotation | 6 + 2 for its tick | smallest three of the unit quaternion at 15 bits each ([`PackedRotation`]) | under 0.009° |
//! | translation, scale | 6 + 2 | xyz unorm16 across the track's own range ([`VectorRange`]) | `extent / 131 070` per axis: 15 µm for hips moving 2 m |
//! | morph weight | 2 + 2 | unorm16 across the track's own range ([`WeightRange`]) | `extent / 131 070` |
//!
//! **Decisions.** Fifteen bits a component against 16 leave room for the two bits that name the
//! dropped component in a 6-byte key: the lost bit doubles a key's worst case from about 0.0045° to
//! 0.009°, which is 0.15 mm at a meter's reach, under the 0.25 mm the cooker's default allows a
//! track. A range per track rather than per clip, since a hand's small motion would otherwise
//! be quantized across the hips' large one. No segments or per-track bit rates, as the
//! Animation Compression Library has: they would save perhaps a third again, and every read
//! would unpack bits. Packs compress clips on top (lz4), and a pack's clips stay small.
//!
//! # Root motion
//!
//! A clip with [`Clip::root`] has its root bone's motion lifted into a track of its own, a
//! transform from the clip's first tick to each later one: rotation (a turn about +Y) then
//! translation, in model space. The root bone's own tracks keep the rest. So
//! `root(t) × pose(t)` is the motion as authored. The simulation reads tables baked from this
//! track rather than the clip (spec §5.13), and the graph ignores it where a move's root motion
//! doesn't apply.
//!
//! # Reading
//!
//! [`Clip::read`] checks the envelope, validates the archive with bytecheck, then checks what a
//! sampler relies on ([`ClipError`]): ranges inside the key arrays, ticks increasing from 0 to the
//! clip's end, tracks in order and naming joints and targets that exist, every rotation key
//! decoding to a unit quaternion. After that a sampler needs no checks of its own.

use bevy_reflect::TypePath;
use bytemuck::{Pod, Zeroable};
use rkyv::rend::u16_le;
use sapphire_reflect::Versioned;
use sapphire_serialize::cooked::{AlignedVec, CookedError};

use crate::AssetId;
use crate::mesh::ArchivedMesh;
use crate::mesh::encoding::packed;
use crate::skeleton::ArchivedSkeleton;

/// Conventional file extension for cooked clips.
pub const CLIP_EXTENSION: &str = "sclip";

/// A clip's rate unless its source says otherwise: the simulation's tick, per second.
pub const DEFAULT_RATE: u16 = 60;

/// The highest [`Clip::rate`].
pub const MAX_RATE: u16 = 960;

/// The longest [`Clip::ticks`]: every tick keeps a key at most, and a track counts its keys in a
/// u16. Eighteen minutes at 60 ticks a second.
pub const MAX_TICKS: u16 = u16::MAX - 1;

/// The largest value a quaternion's three smallest components can take.
const SMALLEST_THREE_MAX: f32 = core::f32::consts::FRAC_1_SQRT_2;
/// One step of a rotation key's 15-bit component.
const ROTATION_STEP: f32 = 2.0 * SMALLEST_THREE_MAX / 32767.0;
/// The largest unorm16 value, as a float.
const UNORM16_MAX: f32 = 65535.0;

/// A cooked clip. See the [module docs](self).
#[derive(Clone, Debug, Default, PartialEq, TypePath, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
#[type_path = "sapphire::assets"]
pub struct Clip {
    /// The source's name for it, for messages and tools.
    pub name: String,
    /// The skeleton whose joints the tracks index; `None` for a clip of morph weights alone on a
    /// mesh with no skeleton.
    pub skeleton: Option<AssetId>,
    /// Joints in [`Self::skeleton`], which [`ArchivedClip::check_skeleton`] confirms a pairing by.
    pub joint_count: u32,
    /// Morph targets in the mesh it was cooked with, which [`ArchivedClip::check_mesh`] confirms
    /// a pairing by; 0 for a clip with no weights.
    pub morph_target_count: u32,
    /// Ticks per second.
    pub rate: u16,
    /// How long it is. 0 is a held pose: a single instant.
    pub ticks: u16,
    /// By increasing joint, each at most once.
    pub rotations: Vec<RotationTrack>,
    /// By increasing joint, each at most once.
    pub translations: Vec<VectorTrack>,
    /// By increasing joint, each at most once.
    pub scales: Vec<VectorTrack>,
    /// By increasing target, each at most once.
    pub weights: Vec<WeightTrack>,
    pub root: Option<RootMotion>,
    /// Every rotation track's keys' ticks, parallel to [`Self::rotation_keys`].
    pub rotation_ticks: Vec<u16>,
    pub rotation_keys: Vec<PackedRotation>,
    /// Every translation and scale track's keys' ticks, parallel to [`Self::vector_keys`].
    pub vector_ticks: Vec<u16>,
    pub vector_keys: Vec<PackedVector>,
    /// Every weight track's keys' ticks, parallel to [`Self::weight_keys`].
    pub weight_ticks: Vec<u16>,
    pub weight_keys: Vec<u16>,
}

impl Versioned for Clip {
    /// 1: the first (M2 N1).
    const VERSION: u32 = 1;
}

/// One joint's rotation over a clip.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct RotationTrack {
    pub joint: u16,
    /// How many keys, at least one.
    pub count: u16,
    /// Its first key in the channel's arrays.
    pub first: u32,
}

/// One joint's translation or scale over a clip.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct VectorTrack {
    pub joint: u16,
    /// How many keys, at least one.
    pub count: u16,
    /// Its first key in the channel's arrays.
    pub first: u32,
    pub range: VectorRange,
}

/// One morph target's weight over a clip.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct WeightTrack {
    /// An index into the mesh's morph targets.
    pub target: u16,
    /// How many keys, at least one.
    pub count: u16,
    /// Its first key in the channel's arrays.
    pub first: u32,
    pub range: WeightRange,
}

/// A clip's root motion (see the [module docs](self)). Both tracks start at the identity, and
/// their `joint` is the root bone.
#[derive(Clone, Copy, Debug, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct RootMotion {
    /// The joint the motion was lifted from.
    pub joint: u16,
    pub translation: VectorTrack,
    /// A turn about +Y.
    pub rotation: RotationTrack,
}

packed!(
    /// A unit quaternion in 48 bits: the three components other than the largest, each 15 bits
    /// across `±√½`, and in the top bits of the first two words which component that was. The
    /// largest is rebuilt as positive, since `q` and `−q` are one rotation; the top bit of the
    /// third word is reserved and zero. Decoding always gives a unit quaternion, off the original
    /// by under 0.009°.
    PackedRotation,
    6
);

impl PackedRotation {
    /// Encodes a unit quaternion, `[x, y, z, w]`.
    pub fn encode(rotation: [f32; 4]) -> Self {
        let mut largest = 0;
        for (index, component) in rotation.iter().enumerate() {
            if component.abs() > rotation[largest].abs() {
                largest = index;
            }
        }
        let sign = if rotation[largest] < 0.0 { -1.0 } else { 1.0 };
        let mut words = [0u16; 3];
        let mut out = words.iter_mut();
        for (index, component) in rotation.iter().enumerate() {
            if index != largest {
                let unit = ((component * sign + SMALLEST_THREE_MAX) / ROTATION_STEP).round();
                *out.next().expect("three components besides the largest") = unit.clamp(0.0, 32767.0) as u16;
            }
        }
        words[0] |= ((largest >> 1) as u16) << 15;
        words[1] |= ((largest & 1) as u16) << 15;
        let [a, b] = words[0].to_le_bytes();
        let [c, d] = words[1].to_le_bytes();
        let [e, f] = words[2].to_le_bytes();
        Self::from_bytes([a, b, c, d, e, f])
    }

    /// The little-endian words.
    #[inline]
    fn words(self) -> [u16; 3] {
        let [a, b, c, d, e, f] = self.to_bytes();
        [u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]), u16::from_le_bytes([e, f])]
    }

    /// The three small components, and which one the largest is.
    #[inline]
    fn parts(self) -> ([f32; 3], usize) {
        let words = self.words();
        let largest = usize::from((words[0] >> 15) << 1 | words[1] >> 15);
        let small = |word: u16| f32::from(word & 0x7fff) * ROTATION_STEP - SMALLEST_THREE_MAX;
        ([small(words[0]), small(words[1]), small(words[2])], largest)
    }

    /// The quaternion, `[x, y, z, w]`, of unit length.
    #[inline]
    #[must_use]
    pub fn decode(self) -> [f32; 4] {
        let ([a, b, c], largest) = self.parts();
        let d = (1.0 - a * a - b * b - c * c).max(0.0).sqrt();
        match largest {
            0 => [d, a, b, c],
            1 => [a, d, b, c],
            2 => [a, b, d, c],
            _ => [a, b, c, d],
        }
    }

    /// Whether this holds a unit quaternion: the reserved bit clear, and room left for the
    /// largest component.
    fn is_valid(self) -> bool {
        let ([a, b, c], _) = self.parts();
        self.words()[2] >> 15 == 0 && a * a + b * b + c * c <= 1.0
    }
}

packed!(
    /// A translation or a scale, as xyz unorm16 across its track's [`VectorRange`].
    PackedVector,
    6
);

impl PackedVector {
    pub fn new([x, y, z]: [u16; 3]) -> Self {
        let [a, b] = x.to_le_bytes();
        let [c, d] = y.to_le_bytes();
        let [e, f] = z.to_le_bytes();
        Self::from_bytes([a, b, c, d, e, f])
    }

    #[inline]
    pub fn get(self) -> [u16; 3] {
        let [a, b, c, d, e, f] = self.to_bytes();
        [u16::from_le_bytes([a, b]), u16::from_le_bytes([c, d]), u16::from_le_bytes([e, f])]
    }
}

/// What a track's unorm16 keys span, per axis: `min + key × step`. A step of 0 is an axis that
/// holds still at `min`, exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct VectorRange {
    pub min: [f32; 3],
    pub step: [f32; 3],
}

impl VectorRange {
    /// The tightest range holding every value of `values`.
    pub fn enclosing(values: impl IntoIterator<Item = [f32; 3]>) -> Self {
        let mut bounds: Option<([f32; 3], [f32; 3])> = None;
        for value in values {
            let (min, max) = bounds.get_or_insert((value, value));
            for axis in 0..3 {
                min[axis] = min[axis].min(value[axis]);
                max[axis] = max[axis].max(value[axis]);
            }
        }
        bounds.map_or_else(Self::default, |(min, max)| Self {
            min,
            step: [0, 1, 2].map(|axis| (max[axis] - min[axis]) / UNORM16_MAX),
        })
    }

    /// The nearest key to `value`, which should lie inside the range (it is clamped).
    pub fn encode(&self, value: [f32; 3]) -> PackedVector {
        PackedVector::new([0, 1, 2].map(|axis| to_unorm16(value[axis], self.min[axis], self.step[axis])))
    }

    /// The value `key` holds.
    #[must_use]
    pub fn decode(&self, key: PackedVector) -> [f32; 3] {
        let key = key.get();
        [0, 1, 2].map(|axis| self.min[axis] + f32::from(key[axis]) * self.step[axis])
    }
}

impl ArchivedVectorRange {
    /// The value `key` holds.
    #[inline]
    #[must_use]
    pub fn decode(&self, key: PackedVector) -> [f32; 3] {
        let key = key.get();
        [0, 1, 2].map(|axis| self.min[axis].to_native() + f32::from(key[axis]) * self.step[axis].to_native())
    }
}

/// What a weight track's unorm16 keys span: `min + key × step`. A step of 0 holds still at `min`.
#[derive(Clone, Copy, Debug, Default, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
#[rkyv(derive(Debug))]
pub struct WeightRange {
    pub min: f32,
    pub step: f32,
}

impl WeightRange {
    /// The tightest range holding every value of `values`.
    pub fn enclosing(values: impl IntoIterator<Item = f32>) -> Self {
        let mut bounds: Option<(f32, f32)> = None;
        for value in values {
            let (min, max) = bounds.get_or_insert((value, value));
            *min = min.min(value);
            *max = max.max(value);
        }
        bounds.map_or_else(Self::default, |(min, max)| Self { min, step: (max - min) / UNORM16_MAX })
    }

    /// The nearest key to `value`, which should lie inside the range (it is clamped).
    pub fn encode(&self, value: f32) -> u16 {
        to_unorm16(value, self.min, self.step)
    }

    /// The value `key` holds.
    #[must_use]
    pub fn decode(&self, key: u16) -> f32 {
        self.min + f32::from(key) * self.step
    }
}

impl ArchivedWeightRange {
    /// The value `key` holds.
    #[inline]
    #[must_use]
    pub fn decode(&self, key: u16) -> f32 {
        self.min.to_native() + f32::from(key) * self.step.to_native()
    }
}

fn to_unorm16(value: f32, min: f32, step: f32) -> u16 {
    if step > 0.0 { ((value - min) / step).round().clamp(0.0, UNORM16_MAX) as u16 } else { 0 }
}

/// The blend of unit quaternions `a` and `b` along the short way around, by `u` and renormalized:
/// what a sampler does between two keys, and what the cook's reduction measures.
#[inline]
#[must_use]
pub fn nlerp(a: [f32; 4], b: [f32; 4], u: f32) -> [f32; 4] {
    let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3];
    let toward = if dot < 0.0 { -u } else { u };
    let from = 1.0 - u;
    let blended = [0, 1, 2, 3].map(|axis| a[axis] * from + b[axis] * toward);
    let length = blended.iter().map(|c| c * c).sum::<f32>().sqrt();
    // Opposite keys blended halfway have no direction: stay on the first.
    if length > 0.0 { blended.map(|c| c / length) } else { a }
}

/// The linear blend of `a` and `b` by `u`.
#[inline]
#[must_use]
pub fn lerp3(a: [f32; 3], b: [f32; 3], u: f32) -> [f32; 3] {
    [0, 1, 2].map(|axis| a[axis] + (b[axis] - a[axis]) * u)
}

/// Where a position falls among a track's keys, as offsets into the track.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeySpan {
    pub from: usize,
    pub to: usize,
    /// How far from `from` to `to`, 0 to 1.
    pub blend: f32,
}

/// The keys around `position` among a track's ticks: at least one, increasing, starting at 0. A
/// position past the last holds it.
#[inline]
#[must_use]
pub fn key_span(ticks: &[u16_le], position: f32) -> KeySpan {
    // The last key at or before the position, by comparisons alone: the branch per step is one a
    // predictor can't learn, so it is a conditional move rather than a jump where it can be.
    let tick = position as u32;
    let mut from = 0;
    let mut length = ticks.len();
    while length > 1 {
        let half = length / 2;
        from += usize::from(u32::from(ticks[from + half].to_native()) <= tick) * half;
        length -= half;
    }
    let to = (from + 1).min(ticks.len() - 1);
    let (start, end) = (f32::from(ticks[from].to_native()), f32::from(ticks[to].to_native()));
    let blend = if to == from { 0.0 } else { ((position - start) / (end - start)).clamp(0.0, 1.0) };
    KeySpan { from, to, blend }
}

/// A cooked clip that can't be read, or breaks an invariant of the format.
#[derive(Debug, thiserror::Error)]
pub enum ClipError {
    /// The envelope, the schema version or the archive itself.
    #[error(transparent)]
    Cooked(#[from] CookedError),
    #[error("a rate of {0} ticks a second; it must be from 1 to {MAX_RATE}")]
    Rate(u16),
    #[error("{0} ticks is over the limit of {MAX_TICKS}")]
    Ticks(u16),
    #[error("{joints} joints and a skeleton of {skeleton:?}: a clip with joints names its skeleton")]
    Skeleton { skeleton: Option<AssetId>, joints: u32 },
    #[error("the {channel} tracks' targets don't increase at {target}")]
    TrackOrder { channel: &'static str, target: u16 },
    #[error("a {channel} track names {target}, but there are {count}")]
    Target { channel: &'static str, target: u16, count: u32 },
    #[error("{channel} has {ticks} ticks and {keys} keys: one of each per key")]
    KeyArrays { channel: &'static str, ticks: usize, keys: usize },
    #[error("the {channel} track of {target} has no keys, or keys past the end of its channel's")]
    KeyRange { channel: &'static str, target: u16 },
    #[error(
        "the {channel} track of {target} must hold one key at tick 0, or keys increasing from 0 to the \
         clip's end"
    )]
    KeyTicks { channel: &'static str, target: u16 },
    #[error("the {channel} track of {target} has a range that isn't finite")]
    Range { channel: &'static str, target: u16 },
    #[error("rotation key {key} isn't a unit quaternion")]
    RotationKey { key: usize },
    #[error("root motion must start at the identity, and name the root bone in both its tracks")]
    Root,
    #[error("the clip expects {expected}, but it has {found}")]
    Mismatch { expected: String, found: String },
}

impl Clip {
    /// The clip as cooked bytes: the envelope header, then the archive.
    pub fn to_cooked(&self) -> Result<AlignedVec, CookedError> {
        sapphire_serialize::to_cooked(self)
    }

    /// Reads cooked clip bytes in place and checks them ([`ArchivedClip::validate`]). The bytes
    /// must start 16-byte aligned.
    pub fn read(bytes: &[u8]) -> Result<&ArchivedClip, ClipError> {
        let clip = sapphire_serialize::access::<Self>(bytes)?;
        clip.validate()?;
        Ok(clip)
    }

    /// [`Self::read`], then a copy out of the archive, for tools and tests.
    pub fn from_cooked(bytes: &[u8]) -> Result<Self, ClipError> {
        let archived = Self::read(bytes)?;
        rkyv::deserialize::<Self, rkyv::rancor::Error>(archived)
            .map_err(|error| ClipError::Cooked(error.into()))
    }
}

impl ArchivedClip {
    /// How long it is, in seconds.
    #[must_use]
    pub fn duration(&self) -> f32 {
        f32::from(self.ticks.to_native()) / f32::from(self.rate.to_native())
    }

    /// The position `seconds` into the clip, a tick and a fraction, held at either end. A clock
    /// that counts in ticks and sub-ticks can make one itself.
    #[inline]
    #[must_use]
    pub fn position(&self, seconds: f32) -> f32 {
        let position = seconds * f32::from(self.rate.to_native());
        // Written so that NaN lands on the first tick.
        if position >= 0.0 { position.min(f32::from(self.ticks.to_native())) } else { 0.0 }
    }

    /// A rotation track's value at `position` (from [`Self::position`]): a unit quaternion,
    /// `[x, y, z, w]`.
    #[inline]
    #[must_use]
    pub fn rotation(&self, track: &ArchivedRotationTrack, position: f32) -> [f32; 4] {
        let (first, count) = (track.first.to_native() as usize, usize::from(track.count.to_native()));
        if count == 1 {
            return self.rotation_keys[first].decode();
        }
        let span = key_span(&self.rotation_ticks[first..first + count], position);
        let (from, to) = (self.rotation_keys[first + span.from], self.rotation_keys[first + span.to]);
        nlerp(from.decode(), to.decode(), span.blend)
    }

    /// A translation or scale track's value at `position`.
    #[inline]
    #[must_use]
    pub fn vector(&self, track: &ArchivedVectorTrack, position: f32) -> [f32; 3] {
        let (first, count) = (track.first.to_native() as usize, usize::from(track.count.to_native()));
        if count == 1 {
            return track.range.decode(self.vector_keys[first]);
        }
        let span = key_span(&self.vector_ticks[first..first + count], position);
        let (from, to) = (self.vector_keys[first + span.from], self.vector_keys[first + span.to]);
        lerp3(track.range.decode(from), track.range.decode(to), span.blend)
    }

    /// A weight track's value at `position`.
    #[inline]
    #[must_use]
    pub fn weight(&self, track: &ArchivedWeightTrack, position: f32) -> f32 {
        let (first, count) = (track.first.to_native() as usize, usize::from(track.count.to_native()));
        if count == 1 {
            return track.range.decode(self.weight_keys[first].to_native());
        }
        let span = key_span(&self.weight_ticks[first..first + count], position);
        let from = track.range.decode(self.weight_keys[first + span.from].to_native());
        let to = track.range.decode(self.weight_keys[first + span.to].to_native());
        from + (to - from) * span.blend
    }

    /// Checks every invariant in [`ClipError`]. [`Clip::read`] calls it.
    pub fn validate(&self) -> Result<(), ClipError> {
        let rate = self.rate.to_native();
        if rate == 0 || rate > MAX_RATE {
            return Err(ClipError::Rate(rate));
        }
        let end = self.ticks.to_native();
        if end > MAX_TICKS {
            return Err(ClipError::Ticks(end));
        }
        let (joints, targets) = (self.joint_count.to_native(), self.morph_target_count.to_native());
        let skeleton = self.skeleton.as_ref().copied();
        if skeleton.is_some() != (joints > 0) {
            return Err(ClipError::Skeleton { skeleton, joints });
        }
        let arrays = |channel, ticks: usize, keys: usize| {
            if ticks == keys { Ok(()) } else { Err(ClipError::KeyArrays { channel, ticks, keys }) }
        };
        arrays("rotation", self.rotation_ticks.len(), self.rotation_keys.len())?;
        arrays("translation and scale", self.vector_ticks.len(), self.vector_keys.len())?;
        arrays("weight", self.weight_ticks.len(), self.weight_keys.len())?;
        if let Some(key) = self.rotation_keys.iter().position(|key| !key.is_valid()) {
            return Err(ClipError::RotationKey { key });
        }

        let rotation = |track: &ArchivedRotationTrack| {
            let target = track.joint.to_native();
            let keys = (track.first.to_native(), track.count.to_native());
            check_track("rotation", target, joints, keys, self.rotation_ticks.as_slice(), end)
        };
        let vector = |channel, track: &ArchivedVectorTrack| {
            let target = track.joint.to_native();
            let keys = (track.first.to_native(), track.count.to_native());
            check_track(channel, target, joints, keys, self.vector_ticks.as_slice(), end)?;
            let range = track.range.min.iter().chain(&track.range.step).map(|c| c.to_native());
            if range.chain(top(&track.range)).all(f32::is_finite) {
                Ok(())
            } else {
                Err(ClipError::Range { channel, target })
            }
        };
        in_order("rotation", self.rotations.iter().map(|track| track.joint.to_native()))?;
        in_order("translation", self.translations.iter().map(|track| track.joint.to_native()))?;
        in_order("scale", self.scales.iter().map(|track| track.joint.to_native()))?;
        in_order("weight", self.weights.iter().map(|track| track.target.to_native()))?;
        self.rotations.iter().try_for_each(rotation)?;
        self.translations.iter().try_for_each(|track| vector("translation", track))?;
        self.scales.iter().try_for_each(|track| vector("scale", track))?;
        for track in self.weights.iter() {
            let target = track.target.to_native();
            let keys = (track.first.to_native(), track.count.to_native());
            check_track("weight", target, targets, keys, self.weight_ticks.as_slice(), end)?;
            let (min, step) = (track.range.min.to_native(), track.range.step.to_native());
            if !(min.is_finite() && step.is_finite() && (min + UNORM16_MAX * step).is_finite()) {
                return Err(ClipError::Range { channel: "weight", target });
            }
        }
        if let Some(root) = self.root.as_ref() {
            let joint = root.joint.to_native();
            if joint != root.translation.joint.to_native() || joint != root.rotation.joint.to_native() {
                return Err(ClipError::Root);
            }
            rotation(&root.rotation)?;
            vector("root motion", &root.translation)?;
            let at_start = self.vector(&root.translation, 0.0);
            let turn = self.rotation(&root.rotation, 0.0);
            // A key rounds to the range's own steps, however long the range is.
            let far = |c: f32, step: f32| c.abs() > ROOT_START_TOLERANCE.max(step);
            let steps = root.translation.range.step.each_ref().map(|step| step.to_native());
            if (0..3).any(|axis| far(at_start[axis], steps[axis])) || turn[..3].iter().any(|&c| far(c, 0.0)) {
                return Err(ClipError::Root);
            }
        }
        Ok(())
    }

    /// Checks that `skeleton` is the one this clip was cooked for, as far as its tracks go: the
    /// same number of joints.
    pub fn check_skeleton(&self, skeleton: &ArchivedSkeleton) -> Result<(), ClipError> {
        let (expected, found) = (self.joint_count.to_native() as usize, skeleton.len());
        if expected == found {
            return Ok(());
        }
        Err(ClipError::Mismatch {
            expected: format!("a skeleton of {expected} joints"),
            found: found.to_string(),
        })
    }

    /// Checks that `mesh` is the one this clip's weights were cooked for: the same number of
    /// morph targets. A clip with no weights fits any mesh.
    pub fn check_mesh(&self, mesh: &ArchivedMesh) -> Result<(), ClipError> {
        let (expected, found) = (self.morph_target_count.to_native() as usize, mesh.morph_targets.len());
        if self.weights.is_empty() || expected == found {
            return Ok(());
        }
        Err(ClipError::Mismatch {
            expected: format!("a mesh of {expected} morph targets"),
            found: found.to_string(),
        })
    }
}

/// How far root motion's first value may be from the identity: what a key's rounding leaves of it,
/// past a translation's own step, and of a rotation.
const ROOT_START_TOLERANCE: f32 = 0.01;

/// Each axis's highest value in `range`, which must be finite too.
fn top(range: &ArchivedVectorRange) -> [f32; 3] {
    [0, 1, 2].map(|axis| range.min[axis].to_native() + UNORM16_MAX * range.step[axis].to_native())
}

fn in_order(channel: &'static str, targets: impl Iterator<Item = u16>) -> Result<(), ClipError> {
    let mut previous = None;
    for target in targets {
        if previous.is_some_and(|previous| previous >= target) {
            return Err(ClipError::TrackOrder { channel, target });
        }
        previous = Some(target);
    }
    Ok(())
}

fn check_track(
    channel: &'static str,
    target: u16,
    targets: u32,
    (first, count): (u32, u16),
    ticks: &[u16_le],
    end: u16,
) -> Result<(), ClipError> {
    if u32::from(target) >= targets {
        return Err(ClipError::Target { channel, target, count: targets });
    }
    let start = first as usize;
    let Some(keys) =
        ticks.get(start..start.saturating_add(usize::from(count))).filter(|keys| !keys.is_empty())
    else {
        return Err(ClipError::KeyRange { channel, target });
    };
    let bad = || ClipError::KeyTicks { channel, target };
    if keys[0].to_native() != 0 {
        return Err(bad());
    }
    if keys.len() > 1 {
        if keys[keys.len() - 1].to_native() != end {
            return Err(bad());
        }
        if keys.windows(2).any(|pair| pair[0].to_native() >= pair[1].to_native()) {
            return Err(bad());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skeleton::{Joint, JointTransform, Skeleton};

    /// A small deterministic generator, so the property tests repeat.
    struct Random(u64);

    impl Random {
        fn unit(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 40) as f32 / (1u64 << 24) as f32
        }

        fn signed(&mut self) -> f32 {
            self.unit() * 2.0 - 1.0
        }

        fn quaternion(&mut self) -> [f32; 4] {
            let q = [self.signed(), self.signed(), self.signed(), self.signed()];
            let length = q.iter().map(|c| c * c).sum::<f32>().sqrt().max(1e-3);
            q.map(|c| c / length)
        }
    }

    /// The angle between two rotations, in radians.
    fn angle(a: [f32; 4], b: [f32; 4]) -> f32 {
        let sign = if a.iter().zip(&b).map(|(a, b)| a * b).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
        let chord = a.iter().zip(&b).map(|(a, b)| (a - b * sign).powi(2)).sum::<f32>().sqrt();
        4.0 * (chord * 0.5).asin()
    }

    #[test]
    fn rotation_keys_stay_within_their_stated_error_and_are_unit() {
        let mut random = Random(7);
        let mut worst: f32 = 0.0;
        for _ in 0..100_000 {
            let q = random.quaternion();
            let key = PackedRotation::encode(q);
            assert!(key.is_valid(), "{q:?}");
            let decoded = key.decode();
            let length = decoded.iter().map(|c| c * c).sum::<f32>();
            assert!((length - 1.0).abs() < 1e-5, "{decoded:?}");
            worst = worst.max(angle(q, decoded));
        }
        assert!(worst.to_degrees() < 0.009, "{}°", worst.to_degrees());
        // The largest component's sign doesn't survive, but the rotation does.
        let flipped = PackedRotation::encode([0.0, 0.0, 0.0, -1.0]).decode();
        assert!(angle(flipped, [0.0, 0.0, 0.0, 1.0]) < 1e-3);
    }

    #[test]
    fn every_largest_component_round_trips() {
        for q in [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]] {
            assert!(angle(PackedRotation::encode(q).decode(), q) < 1e-4, "{q:?}");
        }
    }

    #[test]
    fn ranges_hold_their_ends_and_a_still_axis_exactly() {
        let values = [[0.25, -3.0, 1.0], [0.75, 5.0, 1.0], [0.5, 1.0, 1.0]];
        let range = VectorRange::enclosing(values);
        assert_eq!(range.step[2], 0.0);
        for value in values {
            let decoded = range.decode(range.encode(value));
            assert_eq!(decoded[2], 1.0, "a still axis has no rounding at all");
            for axis in 0..3 {
                assert!(
                    (decoded[axis] - value[axis]).abs() <= range.step[axis] * 0.51,
                    "{decoded:?} {value:?}"
                );
            }
        }
        let weights = WeightRange::enclosing([0.0, 0.5, 1.0]);
        assert_eq!(weights.encode(0.0), 0);
        assert_eq!(weights.encode(1.0), u16::MAX);
        assert!((weights.decode(weights.encode(0.5)) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn nlerp_takes_the_short_way_and_stays_unit() {
        let a = [0.0, 0.0, 0.0, 1.0];
        let half_turn = core::f32::consts::FRAC_1_SQRT_2;
        let b = [0.0, half_turn, 0.0, half_turn];
        let negated = b.map(|c| -c);
        let (forward, around) = (nlerp(a, b, 0.5), nlerp(a, negated, 0.5));
        assert!(angle(forward, around) < 1e-6, "{forward:?} {around:?}");
        assert!(angle(nlerp(a, b, 0.0), a) < 1e-6);
        assert!(angle(nlerp(a, b, 1.0), b) < 1e-6);
        assert_eq!(nlerp(a, a.map(|c| -c), 0.5), a, "opposites hold still rather than divide by zero");
    }

    #[test]
    fn a_span_finds_the_keys_around_a_position() {
        let ticks: Vec<u16_le> = [0u16, 4, 10, 30].iter().map(|&tick| u16_le::from_native(tick)).collect();
        let span = |position| key_span(&ticks, position);
        assert_eq!(span(0.0), KeySpan { from: 0, to: 1, blend: 0.0 });
        assert_eq!(span(2.0), KeySpan { from: 0, to: 1, blend: 0.5 });
        assert_eq!(span(4.0), KeySpan { from: 1, to: 2, blend: 0.0 });
        assert_eq!(span(9.5), KeySpan { from: 1, to: 2, blend: 5.5 / 6.0 });
        assert_eq!(span(20.0), KeySpan { from: 2, to: 3, blend: 0.5 });
        assert_eq!(span(30.0), KeySpan { from: 3, to: 3, blend: 0.0 });
        assert_eq!(span(99.0), KeySpan { from: 3, to: 3, blend: 0.0 }, "holds the end");
        let one = [u16_le::from_native(0)];
        assert_eq!(key_span(&one, 5.0), KeySpan { from: 0, to: 0, blend: 0.0 });
    }

    /// Every position between every pair of ticks agrees with a linear scan.
    #[test]
    fn a_span_agrees_with_a_scan() {
        let mut random = Random(11);
        for length in 1..40usize {
            let mut ticks = vec![0u16];
            while ticks.len() < length {
                let next = ticks[ticks.len() - 1] + 1 + (random.unit() * 5.0) as u16;
                ticks.push(next);
            }
            let packed: Vec<u16_le> = ticks.iter().map(|&tick| u16_le::from_native(tick)).collect();
            let end = *ticks.last().unwrap();
            for step in 0..=u32::from(end) * 4 {
                let position = step as f32 / 4.0;
                let span = key_span(&packed, position);
                let from = ticks.iter().rposition(|&tick| f32::from(tick) <= position).unwrap();
                assert_eq!(span.from, from, "{ticks:?} at {position}");
                assert_eq!(span.to, (from + 1).min(ticks.len() - 1));
            }
        }
    }

    /// A clip with one of every track: a joint turning and moving, one scaling, one morph
    /// target, and root motion.
    fn sample() -> Clip {
        let translation = VectorRange::enclosing([[0.0, 1.0, 0.0], [0.0, 1.5, 2.0]]);
        let scale = VectorRange::enclosing([[2.0, 2.0, 2.0]]);
        let weight = WeightRange::enclosing([0.0, 1.0]);
        let root = VectorRange::enclosing([[0.0, 0.0, 0.0], [0.0, 0.0, 3.0]]);
        let half_turn = core::f32::consts::FRAC_1_SQRT_2;
        Clip {
            name: "sample".into(),
            skeleton: Some(AssetId::from_bytes([1; 16])),
            joint_count: 3,
            morph_target_count: 2,
            rate: DEFAULT_RATE,
            ticks: 30,
            rotations: vec![
                RotationTrack { joint: 0, count: 2, first: 0 },
                RotationTrack { joint: 2, count: 1, first: 2 },
            ],
            translations: vec![VectorTrack { joint: 0, count: 2, first: 0, range: translation }],
            scales: vec![VectorTrack { joint: 1, count: 1, first: 2, range: scale }],
            weights: vec![WeightTrack { target: 1, count: 2, first: 0, range: weight }],
            root: Some(RootMotion {
                joint: 0,
                translation: VectorTrack { joint: 0, count: 2, first: 3, range: root },
                rotation: RotationTrack { joint: 0, count: 1, first: 3 },
            }),
            rotation_ticks: vec![0, 30, 0, 0],
            rotation_keys: vec![
                PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
                PackedRotation::encode([0.0, half_turn, 0.0, half_turn]),
                PackedRotation::encode([half_turn, 0.0, 0.0, half_turn]),
                PackedRotation::encode([0.0, 0.0, 0.0, 1.0]),
            ],
            vector_ticks: vec![0, 30, 0, 0, 30],
            vector_keys: vec![
                translation.encode([0.0, 1.0, 0.0]),
                translation.encode([0.0, 1.5, 2.0]),
                scale.encode([2.0, 2.0, 2.0]),
                root.encode([0.0, 0.0, 0.0]),
                root.encode([0.0, 0.0, 3.0]),
            ],
            weight_ticks: vec![0, 30],
            weight_keys: vec![weight.encode(0.0), weight.encode(1.0)],
        }
    }

    #[test]
    fn a_clip_reads_in_place_and_samples() {
        let clip = sample();
        let bytes = clip.to_cooked().unwrap();
        let archived = Clip::read(&bytes).unwrap();
        assert_eq!(Clip::from_cooked(&bytes).unwrap(), clip);
        assert_eq!(archived.duration(), 0.5);
        assert_eq!(archived.position(0.25), 15.0);
        assert_eq!(archived.position(-1.0), 0.0);
        assert_eq!(archived.position(99.0), 30.0);
        assert_eq!(archived.position(f32::NAN), 0.0);

        let rotation = |position| archived.rotation(&archived.rotations[0], position);
        assert!(angle(rotation(0.0), [0.0, 0.0, 0.0, 1.0]) < 1e-4);
        let quarter = (22.5f32.to_radians().sin(), 22.5f32.to_radians().cos());
        assert!(
            angle(rotation(15.0), [0.0, quarter.0, 0.0, quarter.1]) < 1e-3,
            "halfway: {:?}",
            rotation(15.0)
        );
        assert!(
            angle(archived.rotation(&archived.rotations[1], 7.0), [0.70710677, 0.0, 0.0, 0.70710677]) < 1e-4
        );

        let translation = archived.vector(&archived.translations[0], 15.0);
        assert!(
            (translation[1] - 1.25).abs() < 1e-3 && (translation[2] - 1.0).abs() < 1e-3,
            "{translation:?}"
        );
        assert_eq!(archived.vector(&archived.scales[0], 11.0), [2.0, 2.0, 2.0]);
        assert!((archived.weight(&archived.weights[0], 15.0) - 0.5).abs() < 1e-4);

        let root = archived.root.as_ref().unwrap();
        assert_eq!(archived.vector(&root.translation, 0.0), [0.0, 0.0, 0.0]);
        assert!((archived.vector(&root.translation, 15.0)[2] - 1.5).abs() < 1e-3);
    }

    /// Each row breaks one invariant and must come back as its error, whatever the others hold.
    #[test]
    fn a_bad_clip_is_refused() {
        type Edit = fn(&mut Clip);
        type Expected = fn(&ClipError) -> bool;
        let rows: &[(&str, Edit, Expected)] = &[
            ("no rate", |c| c.rate = 0, |e| matches!(e, ClipError::Rate(0))),
            ("too high a rate", |c| c.rate = MAX_RATE + 1, |e| matches!(e, ClipError::Rate(_))),
            ("too long", |c| c.ticks = u16::MAX, |e| matches!(e, ClipError::Ticks(_))),
            ("no skeleton", |c| c.skeleton = None, |e| matches!(e, ClipError::Skeleton { .. })),
            (
                "tracks out of order",
                |c| c.rotations.swap(0, 1),
                |e| matches!(e, ClipError::TrackOrder { channel: "rotation", .. }),
            ),
            (
                "a repeated track",
                |c| c.rotations[1].joint = 0,
                |e| matches!(e, ClipError::TrackOrder { channel: "rotation", .. }),
            ),
            (
                "a joint past the skeleton's",
                |c| c.translations[0].joint = 3,
                |e| matches!(e, ClipError::Target { channel: "translation", target: 3, .. }),
            ),
            (
                "a target past the mesh's",
                |c| c.morph_target_count = 1,
                |e| matches!(e, ClipError::Target { channel: "weight", target: 1, .. }),
            ),
            (
                "ticks and keys of different lengths",
                |c| {
                    c.vector_keys.pop();
                },
                |e| matches!(e, ClipError::KeyArrays { .. }),
            ),
            (
                "a track with no keys",
                |c| c.rotations[0].count = 0,
                |e| matches!(e, ClipError::KeyRange { channel: "rotation", .. }),
            ),
            (
                "keys past the channel's end",
                |c| c.weights[0].first = u32::MAX,
                |e| matches!(e, ClipError::KeyRange { channel: "weight", .. }),
            ),
            (
                "a first key after tick 0",
                |c| c.rotation_ticks[0] = 1,
                |e| matches!(e, ClipError::KeyTicks { channel: "rotation", .. }),
            ),
            (
                "a last key before the end",
                |c| c.weight_ticks[1] = 29,
                |e| matches!(e, ClipError::KeyTicks { channel: "weight", .. }),
            ),
            (
                "ticks that don't increase",
                |c| {
                    c.rotations[0].count = 3;
                    c.rotation_ticks = vec![0, 0, 30, 0];
                },
                |e| matches!(e, ClipError::KeyTicks { channel: "rotation", .. }),
            ),
            (
                "a held key off tick 0",
                |c| c.rotation_ticks[2] = 1,
                |e| matches!(e, ClipError::KeyTicks { channel: "rotation", .. }),
            ),
            (
                "a range that isn't finite",
                |c| c.translations[0].range.min[1] = f32::NAN,
                |e| matches!(e, ClipError::Range { channel: "translation", .. }),
            ),
            (
                "a range whose top isn't finite",
                |c| c.scales[0].range.step[0] = f32::MAX,
                |e| matches!(e, ClipError::Range { channel: "scale", .. }),
            ),
            (
                "a weight range that isn't finite",
                |c| c.weights[0].range.step = f32::INFINITY,
                |e| matches!(e, ClipError::Range { channel: "weight", .. }),
            ),
            (
                "a rotation key with its reserved bit set",
                |c| c.rotation_keys[1] = PackedRotation::from_bytes([0, 0, 0, 0, 0, 0x80]),
                |e| matches!(e, ClipError::RotationKey { key: 1 }),
            ),
            (
                "a rotation key past the unit sphere",
                |c| c.rotation_keys[1] = PackedRotation::from_bytes([0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f]),
                |e| matches!(e, ClipError::RotationKey { key: 1 }),
            ),
            (
                "root motion naming two joints",
                |c| c.root.as_mut().unwrap().translation.joint = 1,
                |e| matches!(e, ClipError::Root),
            ),
            (
                "root motion that starts away",
                |c| {
                    let root = c.root.as_mut().unwrap();
                    c.vector_keys[3] = root.translation.range.encode([0.0, 0.0, 3.0]);
                },
                |e| matches!(e, ClipError::Root),
            ),
            (
                "root motion that starts turned",
                |c| c.rotation_keys[3] = PackedRotation::encode([0.70710677, 0.0, 0.0, 0.70710677]),
                |e| matches!(e, ClipError::Root),
            ),
        ];
        let mut failures = Vec::new();
        for (name, edit, expected) in rows {
            let mut clip = sample();
            edit(&mut clip);
            let bytes = clip.to_cooked().unwrap();
            match Clip::read(&bytes) {
                Err(error) if expected(&error) => {}
                Err(error) => failures.push(format!("{name}: the wrong error: {error}")),
                Ok(_) => failures.push(format!("{name}: accepted")),
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }

    #[test]
    fn a_held_pose_and_a_clip_of_weights_alone_are_valid() {
        let held = Clip {
            skeleton: Some(AssetId::from_bytes([1; 16])),
            joint_count: 1,
            rate: DEFAULT_RATE,
            rotations: vec![RotationTrack { joint: 0, count: 1, first: 0 }],
            rotation_ticks: vec![0],
            rotation_keys: vec![PackedRotation::encode([0.0, 0.0, 0.0, 1.0])],
            ..Clip::default()
        };
        assert_eq!(Clip::read(&held.to_cooked().unwrap()).unwrap().duration(), 0.0);
        let weights = Clip {
            morph_target_count: 1,
            rate: DEFAULT_RATE,
            ticks: 2,
            weights: vec![WeightTrack {
                target: 0,
                count: 1,
                first: 0,
                range: WeightRange::enclosing([1.0]),
            }],
            weight_ticks: vec![0],
            weight_keys: vec![0],
            ..Clip::default()
        };
        Clip::read(&weights.to_cooked().unwrap()).unwrap();
    }

    #[test]
    fn a_pairing_checks_the_counts_the_tracks_index() {
        let bytes = sample().to_cooked().unwrap();
        let clip = Clip::read(&bytes).unwrap();
        let joint = Joint { name: String::new(), parent: None, rest: JointTransform::IDENTITY };
        let fits = Skeleton::new(&vec![joint.clone(); 3]).to_cooked().unwrap();
        clip.check_skeleton(Skeleton::read(&fits).unwrap()).unwrap();
        let short = Skeleton::new(&vec![joint; 2]).to_cooked().unwrap();
        assert!(matches!(
            clip.check_skeleton(Skeleton::read(&short).unwrap()),
            Err(ClipError::Mismatch { .. })
        ));
    }

    /// The same clip makes the same bytes, and reads the same at the same time: the runtime's
    /// determinism starts here (spec §5.13).
    #[test]
    fn the_same_time_gives_the_same_bytes() {
        let (a, b) = (sample().to_cooked().unwrap(), sample().to_cooked().unwrap());
        assert_eq!(a.as_slice(), b.as_slice());
        let clip = Clip::read(&a).unwrap();
        for step in 0..=120 {
            let position = step as f32 / 4.0;
            let first = clip.rotation(&clip.rotations[0], position);
            let second = clip.rotation(&clip.rotations[0], position);
            assert_eq!(first.map(f32::to_bits), second.map(f32::to_bits));
        }
    }

    /// The layouts the module docs state.
    #[test]
    fn keys_are_the_size_the_docs_say() {
        assert_eq!(size_of::<PackedRotation>(), 6);
        assert_eq!(size_of::<PackedVector>(), 6);
        assert_eq!(size_of::<ArchivedRotationTrack>(), 8);
        assert_eq!(size_of::<ArchivedVectorTrack>(), 32);
        assert_eq!(size_of::<ArchivedWeightTrack>(), 16);
    }
}
