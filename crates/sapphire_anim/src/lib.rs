//! Character animation for presentation.
//!
//! Re-exports the shared CPU pose evaluator and owns the jobs and secondary motion that surround it.

mod character;
pub mod face;
mod motion;
mod springs;

pub use character::{AnimStats, evaluate_all};
pub use face::{FaceLayer, FaceOrder, Mouth};
pub use motion::{CharacterMotion, CharacterMotionError};
pub use sapphire_body_pose::graph::{FixedGraphHistory, GraphStepStamp};
pub use sapphire_body_pose::{
    AimGoal, AnimGraph, ArchivedAnimGraph, ArchivedClip, ArchivedJointTransform, ArchivedRootMotion,
    ArchivedRotationTrack, ArchivedSkeleton, ArchivedVectorRange, ArchivedVectorTrack, ArchivedWeightRange,
    ArchivedWeightTrack, ArmBones, AuthoredFootMotion, Bind, BlendPoint, CLIP_EXTENSION, CharacterIk,
    CharacterPose, Clip, ClipError, ClipSource, CombatInput, ConstraintCycle, Constraints, DEFAULT_RATE,
    EyeDrive, EyeRig, FeetInput, FootContact, FootGround, FootPlacement, FootProbe, FootSettings,
    GRAPH_EXTENSION, GraphBindError, GraphBinding, GraphError, GraphInput, GraphIssue, GraphPlayer,
    GraphStats, Grip, GroundHit, Hand, HeldWeapon, IkInput, IkRig, Joint, JointMask, JointTransform, KeySpan,
    LegBones, LocomotionInput, LookAt, LookBones, LookInput, LookSettings, MAX_JOINTS, MAX_RATE, MAX_TICKS,
    NO_PARENT, PackedRotation, PackedVector, RangeMap, ReactionInput, RootMotion, RotationTrack,
    SKELETON_EXTENSION, Skeleton, SkeletonError, SolvePose, Transform, TwoBone, TwoBoneGoal, VectorRange,
    VectorTrack, WeaponPlace, WeightRange, WeightTrack, add, aim, blend, blend_weights, graph, inertia,
    key_span, lerp3, local_to_model, make_delta, nlerp, overlay, overlay_weights, palette, rest_pose,
    root_motion, sample, two_bone,
};
pub use sapphire_body_pose::{AuthoredHeadLook, GroundTurn, SolvedBody, graph_foot_contact};
pub use sapphire_body_pose::{BodyRoot, BodySample, BodyStep, BodyStepError, FixedBody, FixedBodyPolicy};
pub use sapphire_body_pose::{BodyTuning, BodyTuningError};
pub use springs::{
    ClearanceQuality, ClothError, ClothSheet, HeroineClearance, LONGEST_STRIDE, MAX_CLOTH_ITERATIONS,
    SPRING_TICK_HZ, SpringBones, SpringCollider, SpringFloor, SpringFrame, SpringPace, SpringQuality,
    SpringTail, SpringTuning, SpringWork, StaggerQuality, Substeps,
};
