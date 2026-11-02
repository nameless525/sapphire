//! Character animation (spec §5.13).
//!
//! # Owns
//!
//! Today, the runtime's view of skeletons and clips: the types below, read in place from what the
//! asset server loaded. M2's next package (N2) adds what runs on them.
//!
//! - **[`Skeleton`]:** joints as parallel arrays, parents first, with the rest pose a sampler starts
//!   from.
//! - **[`Clip`]:** one motion for one skeleton, as sparse tracks of quantized keys on a tick grid,
//!   with morph weights and a root motion track. [`ArchivedClip::rotation`] and its neighbors say
//!   what a track holds at a position, without state or allocation, and
//!   [`clip`](sapphire_assets::clip)'s docs give the layout and the reasons for it.
//!
//! # Does not own
//!
//! - **The cooked formats themselves.** They are `sapphire_assets::{skeleton, clip}`, re-exported
//!   here. The asset server checks every kind's bytes before anything reads them
//!   (`AssetKind::references`) and hands them out through a closed set of typed handles, and it
//!   sits a layer below this crate, so a kind's format has to be there too, as a character rig's
//!   already was. This crate is where the runtime names them.
//! - **Importing and key reduction:** the pipeline (tools), which links `sapphire_assets` only.
//! - **The renderer and the world:** no edge to either (rules R3, R4). Poses reach the renderer as
//!   finished palettes in `FramePacket`. The cooker may link this crate, so nothing here may
//!   `include_str!` a shader or a document edited during look-dev (M1 A8's follow-up).
//! - **The simulation:** it never reads a pose. A move's root motion and hit volumes are baked at
//!   cook time into per-tick tables in `sapphire_moves` (spec §5.14, §5.22).
//!
//! # Later in M2
//!
//! - N2: sampling, additive and masked blending, local-to-model, one job per character.
//! - N3: the animation graph and inertialization. N4: IK. N5: spring bones.

pub use sapphire_assets::clip::{
    ArchivedClip, ArchivedRootMotion, ArchivedRotationTrack, ArchivedVectorRange, ArchivedVectorTrack,
    ArchivedWeightRange, ArchivedWeightTrack, CLIP_EXTENSION, Clip, ClipError, DEFAULT_RATE, KeySpan,
    MAX_RATE, MAX_TICKS, PackedRotation, PackedVector, RootMotion, RotationTrack, VectorRange, VectorTrack,
    WeightRange, WeightTrack, key_span, lerp3, nlerp,
};
pub use sapphire_assets::skeleton::{
    ArchivedJointTransform, ArchivedSkeleton, Joint, JointTransform, MAX_JOINTS, NO_PARENT,
    SKELETON_EXTENSION, Skeleton, SkeletonError,
};
