//! Inverse kinematics on a pose (M2 N4, spec §5.13): plain scalar solvers that run between sampling
//! and [`CharacterPose::finish`](crate::CharacterPose::finish), so the rig's twist helpers follow
//! them.

mod solve;

pub use solve::{AimGoal, SolvePose, TwoBone, TwoBoneGoal, aim, two_bone};
