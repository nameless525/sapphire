//! Character animation (spec §5.13).
//!
//! # Owns
//!
//! The runtime's view of skeletons and clips, read in place from what the asset server loaded, and
//! the scalar `glam` core that runs on them:
//!
//! - **[`Skeleton`]:** joints as parallel arrays, parents first, with the rest pose a sampler starts
//!   from.
//! - **[`Clip`]:** one motion for one skeleton, as sparse tracks of quantized keys on a tick grid,
//!   with morph weights and a root motion track. [`ArchivedClip::rotation`] and its neighbors say
//!   what a track holds at a position, without state or allocation, and
//!   [`clip`](sapphire_assets::clip)'s docs give the layout and the reasons for it.
//! - **Plain functions over poses,** each allocation-free and with no state between calls, so the
//!   cooker can call them as the runtime does: [`sample()`], [`overlay`], [`overlay_weights`] and
//!   [`root_motion`]; [`blend()`], [`make_delta`] and [`add`], with [`JointMask`]; [`local_to_model`]
//!   and [`palette`]. A pose is a slice of [`Transform`], one per joint.
//! - **[`Constraints`]:** the rig's twist and aim helpers, which run after the pose.
//! - **[`CharacterPose`]:** one character's frame of it, from sampling to its palette, with
//!   [`evaluate_all`] giving each character a job of its own.
//! - **[`SpringBones`]:** the rig's spring chains and colliders, simulated on the owner's clock beside
//!   a pose (M2 N5).
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
//!   finished palettes in `FramePacket`.
//! - **Whose clock:** callers turn their owner clock's presented time into a position. A clock that
//!   stands still (hitstop, pause) gives the same position, and so the same pose.
//! - **The simulation:** it never reads a pose. A move's root motion and hit volumes are baked at
//!   cook time into per-tick tables in `sapphire_moves` (spec §5.14, §5.22). **The bake doesn't run
//!   this crate:** a table is one set of bytes on every architecture, and these functions' `Quat` and
//!   `Affine3A` round differently on NEON and SSE2, so the pipeline bakes through scalars and a test
//!   there holds a table to the pose this crate draws (spec §5.27).
//!
//! # Later in M2
//!
//! - N3: the animation graph and inertialization. N4: IK.

mod blend;
mod character;
mod constraints;
mod model;
mod sample;
mod springs;

pub use blend::{JointMask, add, blend, make_delta};
pub use character::{AnimStats, CharacterPose, evaluate_all};
pub use constraints::Constraints;
pub use model::{Bind, local_to_model, palette};
pub use sample::{overlay, overlay_weights, rest_pose, root_motion, sample};
pub use sapphire_math::Transform;
pub use springs::{
    DEFAULT_ROOT_INERTIA, SPRING_TICK_HZ, SpringBones, SpringCollider, SpringFrame, SpringQuality,
    SpringTail, SpringWork, Substeps,
};

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
