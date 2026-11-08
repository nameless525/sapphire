//! CPU character pose evaluation.
//!
//! Samples, blends and solves a body from cooked rig and clip data. Callers supply clocks and ground.
//! The fixed-step evaluator publishes constrained body samples for simulation and presentation.

mod blend;
mod blend_space;
mod character;
mod constraints;
pub mod graph;
mod ik;
pub mod inertia;
mod locomotion_history;
mod model;
mod policy;
mod pose_math;
mod sample;

pub use blend::{JointMask, add, blend, make_delta};
pub use blend_space::{BlendPoint, blend_weights};
pub use character::{CharacterPose, JointLayout, SolvedBody};
pub use constraints::{ConstraintCycle, Constraints};
pub use graph::{
    BodyMotionBound, BodyMotionError, BodyReadSpan, BodyRoot, BodySample, BodyStep, BodyStepError, FixedBody,
    FixedBodyPolicy,
};
pub use graph::{
    ClipSource, CombatInput, FixedGraphHistory, GraphBindError, GraphBinding, GraphHistoryError, GraphInput,
    GraphPlayer, GraphStats, GraphStepStamp, LocomotionInput, ReactionInput,
};
pub use ik::{
    AimGoal, ArmBones, AuthoredFootMotion, CharacterIk, EyeDrive, EyeRig, FeetInput, FootContact, FootGround,
    FootPlacement, FootProbe, FootSettings, Grip, GroundHit, Hand, HeldWeapon, IkInput, IkRig, LegBones,
    LookAt, LookBones, LookInput, LookSettings, RangeMap, SolvePose, TwoBone, TwoBoneGoal, WeaponPlace, aim,
    two_bone,
};
pub use locomotion_history::{MotionHistory, MotionObservation};
#[doc(hidden)]
pub use model::model_of;
pub use model::{Bind, local_to_model, palette};
pub use policy::{AuthoredHeadLook, GroundTurn, graph_foot_contact};
pub use sample::{overlay, overlay_weights, rest_pose, root_motion, sample};
pub use sapphire_assets::clip::{
    ArchivedClip, ArchivedRootMotion, ArchivedRotationTrack, ArchivedVectorRange, ArchivedVectorTrack,
    ArchivedWeightRange, ArchivedWeightTrack, CLIP_EXTENSION, Clip, ClipError, DEFAULT_RATE, KeySpan,
    MAX_RATE, MAX_TICKS, PackedRotation, PackedVector, RootMotion, RotationTrack, VectorRange, VectorTrack,
    WeightRange, WeightTrack, key_span, lerp3, nlerp,
};
pub use sapphire_assets::graph::{AnimGraph, ArchivedAnimGraph, GRAPH_EXTENSION, GraphError, GraphIssue};
pub use sapphire_assets::skeleton::{
    ArchivedJointTransform, ArchivedSkeleton, Joint, JointTransform, MAX_JOINTS, NO_PARENT,
    SKELETON_EXTENSION, Skeleton, SkeletonError,
};
pub use sapphire_math::Transform;
