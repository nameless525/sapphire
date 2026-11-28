//! A tick in a glide (M6 A17): what the controller does while [`ControllerState::traversal`] is
//! [`TraversalMode::Gliding`] and the character is in the air.
//!
//! # How it flies
//!
//! - **A steady sink:** every gliding tick falls at [`ResolvedProfile::glide_sink_speed`], the lower of the
//!   profile's `glide_sink_speed` and `max_fall_speed`, from the press that opened it (a rise is dropped too). So a
//!   document whose fastest fall is slower than its glide's sink glides at that fall, and neither number refuses
//!   the other.
//! - **Steering:** a glide always flies forward, along the character's facing, at the profile's `glide_speed`,
//!   reaching it from the speed it had at the profile's `air_acceleration`. The stick only turns it: the facing
//!   turns toward the stick's way at [`TURN_RATE`], 120° a second, whatever the stick's tilt, so turning right
//!   round takes a second and a half, and a 6 m/s glide turns on a circle of 2.9 m radius. A stick let go flies
//!   on straight.
//! - It moves as anyone in the air does: one slide along what it meets (a wall turns it along the wall, and
//!   nothing it meets lifts it), and the ground probe of any airborne tick. A glide casts nothing of its own, so a
//!   gliding tick makes the same casts as a falling one.
//!
//! # Where it ends
//!
//! On landing, on ground to stand on or a slope to slide down, the character is back in
//! [`TraversalMode::None`] and walks or slides from there. A move taking the stick ends it first (an air attack, a
//! hit's reaction), and so does stamina's end: [`ControllerState::stamina_spent`] counts gliding time as it counts
//! a climb's, the two sharing one stamina that comes back whole only when the feet stand on the ground. A press
//! of jump drops it into a fall, which [`crate::traversal`] reads, as it reads the press that opens one.
//!
//! The glide's tick runs only in the air: a mode set on someone on the ground (only a test does that) walks as
//! before, and a press can open a glide only in the air.

use sapphire_math::{Vec3, det};
use sapphire_time::SimDuration;

use super::profile::ResolvedProfile;
use super::{ControllerState, GroundMode, TraversalMode};

/// How fast a glide's facing turns toward the stick, in radians a second: 120°.
pub const TURN_RATE: f32 = 2.0 * std::f32::consts::FRAC_PI_3;

/// Opens a glide: what a press in the air high enough does to the state (M6 A17). The sink starts at once, and the
/// speed across is kept for the first tick to bring round to the facing.
pub(crate) fn open(state: &mut ControllerState, profile: &ResolvedProfile) {
    state.traversal = TraversalMode::Gliding;
    state.mode = GroundMode::Airborne;
    state.velocity.y = -profile.glide_sink_speed();
    state.coyote = SimDuration::ZERO;
    state.jump_waiting = false;
}

/// Ends a glide in the air: it falls from here, with the speed it had.
pub(crate) fn drop(state: &mut ControllerState) {
    state.traversal = TraversalMode::None;
}

/// Whether a gliding tick of `delta` spends the last of the profile's stamina, counting it.
pub(super) fn tires(state: &mut ControllerState, profile: &ResolvedProfile, delta: SimDuration) -> bool {
    let Some(stamina) = profile.stamina() else { return false };
    state.stamina_spent += delta;
    state.stamina_spent >= stamina
}

/// A gliding tick's velocity and facing: `yaw` turned toward `want`'s way if it has one, then the steady sink and
/// the speed across along the facing.
pub(super) fn steer(
    state: &mut ControllerState,
    yaw: &mut f32,
    profile: &ResolvedProfile,
    want: Vec3,
    dt: f32,
) {
    let p = profile.authored();
    if want != Vec3::ZERO {
        *yaw = det::turn_towards(*yaw, det::yaw_of_direction(want), TURN_RATE * dt);
    }
    let speed = Vec3::new(state.velocity.x, 0.0, state.velocity.z).length();
    let step = p.air_acceleration * dt;
    // Exactly the glide's speed once it's in reach, so a steady glide keeps its bits.
    let speed = if (p.glide_speed - speed).abs() <= step {
        p.glide_speed
    } else {
        speed + step.copysign(p.glide_speed - speed)
    };
    state.velocity = det::direction_from_yaw(*yaw) * speed - Vec3::Y * profile.glide_sink_speed();
}
