//! The camera director (spec §5.22): rigs that frame the fighters, sphere-cast collision, critically
//! damped smoothing on the camera's own clock, and shakes as cues.
//!
//! **Presentation, not simulation.** It reads interpolated transforms and feeds nothing back, so it's
//! outside the checksum, and the simulation never reads it: `TickActions`' stick is world-space, rotated
//! by [`CameraDirector::heading`] before it's quantized. It takes plain positions, so it has no
//! edge to `sapphire_gameplay`, and it isn't in `[simulation]`, which is why `std`'s math is fine here.
//! The one exception is the shakes' noise, on `sapphire_math::det` so a seed means the same everywhere.
//! What a soft lock chooses does decide gameplay, so that half of targeting is
//! `sapphire_gameplay::targeting`, and it reaches this crate only as the target's position.
//!
//! # Each frame
//!
//! ```
//! # use sapphire_camera::{CameraDirector, CameraFrame, CameraTuning, Rig, Subject};
//! # use sapphire_math::{Vec2, Vec3};
//! # use sapphire_physics::PhysicsWorld;
//! let world = PhysicsWorld::new(Vec::new(), 0).unwrap();
//! let mut camera = CameraDirector::new(CameraTuning::default_tuning(), 7).unwrap();
//! camera.set_rig(Rig::Combat, true);
//! let player = Subject { position: Vec3::ZERO, height: 1.7, radius: 0.35 };
//! let enemy = Subject { position: Vec3::new(1.0, 0.0, 4.0), height: 2.0, radius: 0.4 };
//!
//! // After the frame's fixed steps, with positions at its interpolated time:
//! let pose = camera.update(
//!     &CameraFrame {
//!         dt: 1.0 / 60.0,        // Real seconds: 0 while paused, unscaled by hitstop or slow motion.
//!         behind: 0.0,           // How far the frame draws behind the latest step, for new shakes.
//!         player,
//!         player_yaw: 0.0,       // Which way the player faces.
//!         target: Some(enemy),   // From `sapphire_gameplay::targeting`, or the app's own choice.
//!         look: Vec2::ZERO,      // `CombatControls::look`: radians of yaw and pitch this frame.
//!         zoom: 0.0,             // `CombatControls::camera_zoom`: the log of a factor on the distance.
//!         aspect: 16.0 / 9.0,
//!     },
//!     &world,
//! );
//!
//! // The view, and the yaw the next tick's stick turns by, in `CombatControls::next_tick`:
//! let view = pose.view();
//! let camera_yaw = camera.heading();
//! # let _ = (view, camera_yaw, pose.fov_y);
//! ```
//!
//! # Rigs
//!
//! A stack, each blending over what's below by a weight that's a critically damped spring toward on or
//! off ([`Rig`], [`CameraDirector::set_rig`]). Between the rigs' goals and the view are springs on the
//! yaw, pitch, pivot, distance and field of view, so velocity stays continuous through anything a rig
//! does: a rig popping on and off in one frame moves its weight by a few percent.
//!
//! - **Explore** follows the player. Look input turns the view; after `idle_delay` with no input the
//!   pitch drifts back to rest, and if the player's moving the yaw drifts behind them.
//!   [`CameraDirector::recenter`] does both at once, until input interrupts it.
//! - **Combat** frames the player and the target. Look input stays free, within what keeps the target in
//!   the safe frame: past that limit the view turns back at `turn_half_life`, no faster than
//!   `max_turn_rate`. The distance is the least that fits both fighters' heads and feet, from the view as
//!   it stands, within the tuning's range. It pulls out quickly and pushes in slowly.
//! - **Lock-on** holds the line from player to target, off to whichever side the view was on. **Look input
//!   only leans it**, within `lean_yaw` and `lean_pitch`, and the lean returns when input lets go. The
//!   game's stick and its target choice don't change: flicking to another target is the app asking
//!   targeting again and passing the new position, which the view follows without a jump.
//! - **Takeover** is a sequencer's authored pose (M3): [`CameraDirector::set_takeover`].
//!
//! Zoom scales every rig's distance, within `zoom_min` and `zoom_max`, and framing still opens past it to
//! fit both fighters.
//!
//! # Its own clock
//!
//! [`CameraFrame::dt`] is the only time it knows (rule R2). Pass real frame seconds, so hitstop and slow
//! motion leave its smoothing and shakes running and the frame reads alive through a freeze. Pass 0 in a
//! pause, which holds the pose exactly.
//!
//! # Smoothing
//!
//! `spring::Follow` is exact for a target moving in a straight line over the step, which is what
//! interpolated positions do, so uneven frames don't judder. The yaw and pitch each have two, one for input
//! and one for what the rigs turn them by, so a quick stick and a slow recentre keep their own half-lives.
//! Any `dt` is stable, and [`CameraDirector::cut`] resets.
//!
//! `tests/frame_rate.rs` states the tolerance: at 30 and 60 Hz against 120, a settled run ends within 2 cm
//! and 0.004 rad, and stays within 12 cm and 0.012 rad along the way. Lock-on's view, at its speed limit
//! after an enemy sprinting round the player, stays within half a frame's turn: 0.05 rad at 30 Hz.
//!
//! # Collision
//!
//! The `collision` module's docs have the method. It casts spheres through `sapphire_physics` against
//! `Group::World | Group::CameraBlocker`: the eye's sweep never enters a collider, a thin occluder is
//! passed behind for a while rather than pumped in front of, and the rest come in early and leave
//! slowly.
//!
//! # Shakes
//!
//! See the `shake` module for E3's interface: [`CameraTuning::shake_id`], [`CameraDirector::fire_shake`]
//! and [`CameraDirector::set_shake_setting`].
//!
//! # Tuning
//!
//! Every number is in [`CameraTuning`], a RON document checked by field, shipped as
//! `default_camera.ron`, and swapped in by [`CameraDirector::set_tuning`] with the camera running.
//!
//! # Allocation
//!
//! Building a director and setting tuning allocate; a frame never does (`tests/allocations.rs`).
//!
//! # Cost
//!
//! It reads no clock, so `examples/camera_bench.rs` times it. In release, on an M5 at a load average of
//! about 20, with 227 colliders and the fighters walking among them, over 7 alternating rounds of 4,000
//! frames: a median frame takes 0.9 µs exploring, 2.0 in combat and 1.6 in lock-on, the 99th percentile is
//! under 4 µs, and the worst about 5 µs, for 5–6 sphere casts a frame.
//!
//! # Does not own
//!
//! - Turning the pose into `CameraData` and the sandbox's wiring: O5 and the combat scene do that.
//! - Who the target is: `sapphire_gameplay::targeting`. The app passes a position.
//! - Which cue fires which shake (E3), and input bindings (I1).
//! - Sequencer timelines (M3): the takeover slot takes their poses.

mod collision;
mod director;
mod rigs;
mod shake;
mod spring;
mod tuning;

pub use director::{CameraDirector, CameraFrame, CameraPose, CameraStats};
pub use rigs::{Rig, Subject};
pub use shake::{MAX_SHAKES, NamedShake, Shake, ShakeId, ShakeStack};
pub use tuning::{
    CameraTuning, CameraTuningError, CollisionTuning, CombatTuning, ExploreTuning, FramingTuning,
    LockOnTuning, LookTuning, ShakeTuning, TakeoverTuning,
};
