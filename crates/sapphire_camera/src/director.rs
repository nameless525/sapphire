//! The director: one call a frame, from plain inputs to a plain pose.

use sapphire_math::det::wrap_angle;
use sapphire_math::projection::view_from_transform;
use sapphire_math::spring::damp;
use sapphire_math::{Mat3, Mat4, Quat, Transform, UP, Vec2, Vec3};
use sapphire_physics::PhysicsWorld;

use crate::clearing::{Clearing, Scene};
use crate::collision::{Arm, CRAMPED};
use crate::rigs::{Axes, Rig, Subject, framed_distance, framed_pivot, smoothstep, yaw_limit};
use crate::shake::{ShakeId, ShakeStack};
use crate::spring::{Follow, chase, rate_of};
use crate::tuning::{CameraTuning, CameraTuningError, FramingTuning, LookTuning};

/// The longest step the camera takes in one frame, in seconds: long past settling, and short enough
/// that nothing it multiplies overflows.
const MAX_STEP: f32 = 3600.0;
/// A weight this near its end is at its end, so a rig that's off costs nothing.
const WEIGHT_SNAP: f32 = 1e-3;
/// How far a goal may jump in a frame and still count as moving: past this, it changed hands, and
/// ramping across the jump would add a lag that means nothing.
const GOAL_JUMP: f32 = 0.5;
/// The most any one rig's pull counts for, in radians: further off than this it turns no faster. It keeps
/// a target circling faster than the view can turn from winding the view up, then flipping it round as
/// the short way changes sides.
const MOST_PULL: f32 = 1.0;
/// Unwrapped yaw is rewrapped past this, in radians, to stay precise.
const YAW_REWRAP: f32 = 256.0;
/// How much of the rig's own direction is in where the view looks, in metres, added to the way from the
/// eye to the pivot. With the eye where the rig puts it this changes nothing, since they're the same
/// direction. With the arm pulled in the eye's off that line, and this keeps a turn of the view in hand: near
/// the pivot, that way swings by radians for centimetres.
pub(crate) const LOOK_STEADY: f32 = 1.5;
/// Close enough to the goal to end a recentring, in radians.
const RECENTERED: f32 = 0.02;
/// How quickly the fighters' velocities are read, as a half-life: long enough to take the edge off a
/// dash's first frame.
const VELOCITY_HALF_LIFE: f32 = 0.05;
/// How far a target may move in a frame and still be the same target moving, at this speed in metres a
/// second or this far in metres, whichever is more. Past that, it's another one.
const TARGET_JUMP: (f32, f32) = (40.0, 1.0);
/// How far ahead clearing looks for one fighter passing behind the other, in seconds: about how long a turn
/// takes to get going.
const LOOK_AHEAD: f32 = 0.5;
/// An eye this near the player's chest looks at who it's framing, in metres, as long as they're no nearer than
/// this either. By [`CRAMPED`] it looks where the rig does.
const SHUT_IN: f32 = 0.6;
/// How softly a view goes over to who it's framing and back, as the arm comes and goes much faster.
const SHUT_IN_HALF_LIFE: f32 = 0.15;
/// A frame this long, in seconds, has no view to judge: it lands where a settled camera stands, and whatever
/// clearing was doing is over.
const LONG_FRAME: f32 = 0.25;
/// How far ahead the framing rigs look for the fighters, as a share of the half-life they pull out with:
/// half of a spring's lag behind a steady goal (`2 / rate`), since the pivot catches up with them meanwhile.
const LEAD_PER_HALF_LIFE: f32 = 1.0 / 1.678_347;

/// How shut in by the player's chest an eye is: 1 at [`SHUT_IN`] or nearer, and 0 by [`CRAMPED`]. It sees
/// nothing where its rig looks, so this much of its view goes to who it's framing, unless they're as close.
pub(crate) fn shut_in(eye: Vec3, anchor: Vec3, framed: Vec3) -> f32 {
    (1.0 - smoothstep(SHUT_IN, CRAMPED, eye.distance(anchor)))
        * smoothstep(SHUT_IN, CRAMPED, eye.distance(framed))
}

/// The way a view looks from `eye`, given where its rig looks, with `share` of it on who it's framing.
pub(crate) fn looking(eye: Vec3, pivot: Vec3, forward: Vec3, framed: Option<Vec3>, share: f32) -> Vec3 {
    let along = pivot - eye + forward * LOOK_STEADY;
    let Some(framed) = framed.filter(|_| share > 0.0) else { return along };
    along.normalize_or(forward).lerp((framed - eye).normalize_or(forward), share)
}

/// One frame's inputs: plain values the app reads from its clock, transforms and input.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraFrame {
    /// The camera's clock: real seconds since the last frame, unscaled, so hitstop and slow motion
    /// don't touch it, and 0 while the game is paused, which holds everything. Never the OS clock read
    /// here (rule R2): the app has it from its frame loop.
    pub dt: f32,
    /// How many seconds this frame draws behind the latest fixed step: `1 - alpha` of a tick, as poses and
    /// transforms do. It places a shake fired since the last frame, so it starts when what's drawn gets
    /// to the step that fired it ([`ShakeStack::advance`]) and then runs on `dt`.
    pub behind: f32,
    pub player: Subject,
    /// Which way the player faces, in radians from +Z toward +X: what recentring turns behind.
    pub player_yaw: f32,
    /// Who the framing rigs frame. Without one they let go.
    pub target: Option<Subject>,
    /// Look input for this frame, in radians, as `sapphire_controls`' `CombatControls::look` gives it:
    /// added to the view's yaw (toward +X, so turning left) and pitch (above zero looks down).
    pub look: Vec2,
    /// Zoom input for this frame, as `CombatControls::camera_zoom` gives it: the natural log of a factor on
    /// the distance, so below zero moves closer. It builds up within the tuning's limits.
    pub zoom: f32,
    /// Width over height, which the framing rigs fit their frame to.
    pub aspect: f32,
}

/// What the camera sees: a plain pose and field of view for the app to turn into a view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPose {
    /// The eye, in world metres.
    pub eye: Vec3,
    /// Turns +Z to where it looks and +Y to its up, as every object's rotation does.
    pub rotation: Quat,
    /// The full vertical field of view, in radians.
    pub fov_y: f32,
}

impl CameraPose {
    /// Where it looks.
    #[must_use]
    pub fn forward(&self) -> Vec3 {
        self.rotation * sapphire_math::FORWARD
    }

    /// World to view space, as `sapphire_math::projection::view_from_transform` builds it.
    #[must_use]
    pub fn view(&self) -> Mat4 {
        view_from_transform(&Transform { translation: self.eye, rotation: self.rotation, scale: Vec3::ONE })
    }
}

/// Work since [`CameraDirector::take_stats`] was last called (rule R9). The camera reads no clock:
/// whoever runs it times it. There's no line for it in §3.2's budget, and its frame cost is in the crate
/// docs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CameraStats {
    /// [`CameraDirector::update`]s that advanced.
    pub frames: u32,
    /// Sphere casts and overlap queries against the physics world.
    pub casts: u32,
    /// Frames with a thin occluder on the line, passed behind.
    pub thin_frames: u32,
    /// Frames with the arm pulled in.
    pub pulled_frames: u32,
    /// Times the eye met a surface and slid.
    pub contacts: u32,
    /// Times the eye couldn't get to its place and jumped: the one exception to the jerk bound.
    pub hops: u32,
    /// Cuts taken, including the first frame's.
    pub cuts: u32,
    /// Times the view chose to turn round what's in its way.
    pub swings: u32,
    pub shakes_fired: u32,
    /// Shakes that replaced another because the stack was full.
    pub shakes_replaced: u32,
}

/// The camera director. See the [crate docs](crate).
#[derive(Clone, Debug)]
pub struct CameraDirector {
    tuning: CameraTuning,
    wanted: [bool; Rig::COUNT],
    /// Each rig's share, 0 to 1, and how fast it's moving.
    weights: [f32; Rig::COUNT],
    weight_speeds: [f32; Rig::COUNT],

    yaw: Angle,
    pitch: Angle,
    /// What look input does under lock-on: a lean off the line, yaw then pitch, in radians.
    lean: Vec2,
    /// Which side of the player the lock-on view stands, +1 or −1.
    side: f32,
    /// Seconds since look input.
    idle: f32,
    recentering: bool,
    /// What zoom has built up to: the natural log of the factor on every rig's distance.
    zoom: f32,
    /// Where the view looks across the ground, before any shake.
    heading: f32,

    pivot_across: Follow<Vec2>,
    pivot_height: Follow<f32>,
    distance: Follow<f32>,
    fov_y: Follow<f32>,
    /// The fighters, followed only to read their velocities.
    player_motion: Follow<Vec3>,
    target_motion: Follow<Vec3>,
    /// The last target seen, which a rig fading out keeps framing.
    last_target: Option<Subject>,
    /// How far the framed target sits from the real one. A new target starts this at the gap to the
    /// old one and it closes, so a switch doesn't jump, and a target's own motion is framed with no lag.
    target_offset: Follow<Vec3>,

    arm: Arm,
    /// How much of the view is on who it's framing, for an eye shut in: [`shut_in`], softened.
    shut_in: Follow<f32>,
    clearing: Clearing,
    shakes: ShakeStack,
    takeover: Option<CameraPose>,
    /// The takeover pose a fade-out blends from once the sequencer stops giving one.
    last_takeover: Option<CameraPose>,

    /// The last valid inputs, which stand in for invalid ones.
    last_frame: Option<CameraFrame>,
    cut_pending: bool,
    pose: CameraPose,
    stats: CameraStats,
}

/// A yaw or a pitch: where input and the rigs have it heading, and the view following.
///
/// What input does and what the rigs do follow on separate springs, so the view answers input quickly while
/// a rig's turn starts and stops softly, however the two mix. Each spring is exact for a straight line, and
/// what feeds them is straight over a step, so the result doesn't depend on the frame rate.
#[derive(Clone, Copy, Debug)]
struct Angle {
    /// Where it's heading, input and rigs together. A yaw isn't wrapped.
    heading: f32,
    /// How much of that the rigs have turned.
    turned: f32,
    /// Where the rigs last asked it to go, while they ask.
    goal: Option<f32>,
    from_input: Follow<f32>,
    from_rigs: Follow<f32>,
}

impl Angle {
    fn new(value: f32) -> Self {
        Self {
            heading: value,
            turned: 0.0,
            goal: None,
            from_input: Follow::new(value),
            from_rigs: Follow::new(0.0),
        }
    }

    /// The angle the view has now.
    fn value(&self) -> f32 {
        self.from_input.value + self.from_rigs.value
    }

    /// Moves it there at once.
    fn set(&mut self, value: f32) {
        *self = Self::new(value);
    }

    /// Advances what follows `heading` by `dt` seconds. With `snap` it lands.
    fn follow(&mut self, look: &LookTuning, dt: f32, snap: bool) -> f32 {
        let (input, rigs) = if snap { (0.0, 0.0) } else { (look.angle_half_life, look.turn_half_life) };
        self.from_input.update(self.heading - self.turned, input, dt);
        self.from_rigs.update(self.turned, rigs, dt);
        self.value()
    }

    /// Moves `heading` by what the rigs pull it for this step, at most `max_speed`.
    fn steer(&mut self, pull: Pull, max_speed: f32, dt: f32, snap: bool) {
        if pull.rate <= 0.0 {
            self.goal = None;
            return;
        }
        let goal = self.heading + pull.turn / pull.rate;
        let step = if snap {
            goal - self.heading
        } else {
            let limit = max_speed * dt;
            if pull.held {
                // Its goal stays ahead of it, so it moves at one speed.
                (pull.turn * dt).clamp(-limit, limit)
            } else {
                let last = self.goal.filter(|from| (goal - from).abs() < GOAL_JUMP);
                let from = last.unwrap_or(goal);
                (chase(self.heading, from, goal, pull.rate, dt) - self.heading).clamp(-limit, limit)
            }
        };
        self.heading += step;
        self.turned += step;
        self.goal = Some(goal);
    }

    /// Keeps a yaw near zero by whole turns, which changes nothing it's used for.
    fn rewrap(&mut self) {
        if self.heading.abs() < YAW_REWRAP {
            return;
        }
        let by = wrap_angle(self.heading) - self.heading;
        self.heading += by;
        self.from_input.shift(by);
        self.goal = self.goal.map(|goal| goal + by);
    }
}

/// A running total of what the rigs want an angle to do: each pulls toward its own goal at its own rate,
/// and together they pull toward the rates' average of their goals at the rates' sum. A rig's rate
/// scales with its weight, so one fading in or out never jumps.
#[derive(Clone, Copy, Debug, Default)]
struct Pull {
    turn: f32,
    rate: f32,
    /// A pull is held to [`MOST_PULL`], so its goal moves along with the angle: not a goal to ramp toward.
    held: bool,
}

impl Pull {
    /// A pull of `turn` radians (the short way to its goal) at `rate` per second.
    fn add(&mut self, turn: f32, rate: f32) {
        self.turn += turn.clamp(-MOST_PULL, MOST_PULL) * rate;
        self.rate += rate;
        self.held |= turn.abs() > MOST_PULL;
    }
}

impl CameraDirector {
    /// A director with only [`Rig::Explore`] on. `seed` picks the shakes' noise. Its first
    /// [`Self::update`] is a cut.
    pub fn new(tuning: CameraTuning, seed: u64) -> Result<Self, CameraTuningError> {
        tuning.validate()?;
        let radius = tuning.collision.radius;
        Ok(Self {
            yaw: Angle::new(0.0),
            pitch: Angle::new(tuning.explore.pitch),
            tuning,
            wanted: [true, false, false, false],
            weights: [1.0, 0.0, 0.0, 0.0],
            weight_speeds: [0.0; Rig::COUNT],
            lean: Vec2::ZERO,
            side: 1.0,
            idle: 0.0,
            recentering: false,
            zoom: 0.0,
            heading: 0.0,
            pivot_across: Follow::new(Vec2::ZERO),
            pivot_height: Follow::new(0.0),
            distance: Follow::new(0.0),
            fov_y: Follow::new(0.0),
            player_motion: Follow::new(Vec3::ZERO),
            target_motion: Follow::new(Vec3::ZERO),
            last_target: None,
            target_offset: Follow::new(Vec3::ZERO),
            arm: Arm::new(radius),
            shut_in: Follow::new(0.0),
            clearing: Clearing::new(),
            shakes: ShakeStack::new(seed),
            takeover: None,
            last_takeover: None,
            last_frame: None,
            cut_pending: true,
            pose: CameraPose { eye: Vec3::ZERO, rotation: Quat::IDENTITY, fov_y: 1.0 },
            stats: CameraStats::default(),
        })
    }

    /// Swaps in new tuning with the camera running, for hot reload. Only goals move, so the view eases
    /// to them; shakes already playing finish as fired. Refused, with the old tuning kept, if a value is
    /// out of range. Shake IDs resolved from the old tuning need resolving again.
    pub fn set_tuning(&mut self, tuning: CameraTuning) -> Result<(), CameraTuningError> {
        tuning.validate()?;
        self.arm.set_radius(tuning.collision.radius);
        self.tuning = tuning;
        Ok(())
    }

    #[must_use]
    pub fn tuning(&self) -> &CameraTuning {
        &self.tuning
    }

    /// Turns a framing rig on or off; it blends over its own half-life. A rig popped on and off within a
    /// frame or two barely moves. [`Rig::Explore`] is always on, and [`Rig::Takeover`] follows
    /// [`Self::set_takeover`], so both ignore this.
    pub fn set_rig(&mut self, rig: Rig, on: bool) {
        if matches!(rig, Rig::Combat | Rig::LockOn) {
            self.wanted[rig as usize] = on;
        }
    }

    /// A rig's current share of the view, 0 to 1. Its weight is a spring, and this eases in and out of
    /// that, so a rig popped on for a frame, whose weight reaches a few percent, has a share of a tenth
    /// of that.
    #[must_use]
    pub fn weight(&self, rig: Rig) -> f32 {
        let weight = self.weights[rig as usize];
        weight * weight * (3.0 - 2.0 * weight)
    }

    /// Gives the top of the stack to an authored pose, updated every frame by a sequencer (M3), or hands
    /// it back with `None`. It blends with the rigs' result over the tuning's half-life. At full weight
    /// the view is exactly this pose, collision aside: a shot's author owns its framing. The rigs keep
    /// running beneath it, so the view they hand back to is current. Shakes still apply on top.
    pub fn set_takeover(&mut self, pose: Option<CameraPose>) {
        self.wanted[Rig::Takeover as usize] = pose.is_some();
        self.takeover = pose;
        if pose.is_some() {
            self.last_takeover = pose;
        }
    }

    /// Asks the view to turn behind the player and level out: a stick click, say. Any look input ends it.
    pub fn recenter(&mut self) {
        self.recentering = true;
    }

    /// Faces the view this way at once, for a spawn or after a cut: `yaw` from +Z toward +X, and `pitch`
    /// down, held to the tuning's limits.
    pub fn look_from(&mut self, yaw: f32, pitch: f32) {
        self.yaw.set(yaw);
        self.pitch.set(pitch.clamp(self.tuning.look.pitch_min, self.tuning.look.pitch_max));
    }

    /// Lands every spring on its goal at the next update, with nothing moving: for a teleport or a
    /// change of shot. A fighter that teleports without this is followed, quickly and smoothly.
    pub fn cut(&mut self) {
        self.cut_pending = true;
    }

    /// Stops every shake at once: what fired them is gone, as when a fight starts over. [`Self::cut`]
    /// leaves them playing, since a change of shot in the middle of a fight doesn't end a hit.
    pub fn stop_shakes(&mut self) {
        self.shakes.clear();
    }

    /// Fires a shake (see the `shake` module for E3's interface). `strength` scales this firing; the
    /// player's setting scales them all. An ID from another tuning, past this one's list, does nothing.
    pub fn fire_shake(&mut self, id: ShakeId, strength: f32) {
        let Some(preset) = self.tuning.shakes.presets.get(usize::from(id.0)) else { return };
        self.stats.shakes_fired += 1;
        if self.shakes.fire(preset.shake, strength) {
            self.stats.shakes_replaced += 1;
        }
    }

    /// The player's shake setting, 0 (none, for accessibility) to 1 (as authored).
    pub fn set_shake_setting(&mut self, setting: f32) {
        self.shakes.set_setting(setting);
    }

    /// Where the view looks across the ground, in radians, as an actor's yaw: +Z at 0, turning toward +X. It's
    /// what the app passes as `camera_yaw` to `CombatControls::next_tick`, which turns the raw stick into
    /// the world's, so a replay carries the direction the player meant and the simulation never reads a
    /// camera. It's the drawn view's, before any shake, so a shake can't move the stick.
    #[must_use]
    pub fn heading(&self) -> f32 {
        self.heading
    }

    /// The pose [`Self::update`] last returned.
    #[must_use]
    pub fn pose(&self) -> CameraPose {
        self.pose
    }

    /// The work done since this was last called.
    pub fn take_stats(&mut self) -> CameraStats {
        std::mem::take(&mut self.stats)
    }

    /// Runs one frame and returns the pose to draw. Call it once a frame, after the fixed steps, with
    /// positions at the frame's interpolated time and before the app builds its view. With `dt` at 0 it
    /// holds the last pose, unless a cut is pending. `world` is only read.
    pub fn update(&mut self, frame: &CameraFrame, world: &PhysicsWorld) -> CameraPose {
        profiling::scope!("CameraDirector::update");
        let frame = self.sanitized(frame);
        let snap = std::mem::take(&mut self.cut_pending);
        let dt = frame.dt.min(MAX_STEP);
        if dt <= 0.0 && !snap {
            return self.pose;
        }
        self.stats.frames += 1;
        if snap {
            self.stats.cuts += 1;
        }

        self.step_weights(&frame, dt, snap);
        if snap {
            self.player_motion.reset(frame.player.position);
        } else {
            self.player_motion.update(frame.player.position, VELOCITY_HALF_LIFE, dt);
        }
        let target = self.step_target(&frame, dt, snap);

        let anchor = frame.player.at_height(self.tuning.explore.pivot_height);
        let shot = self.shot(anchor, target.as_ref());
        let fov_half_life = if snap { 0.0 } else { self.tuning.look.fov_half_life };
        let fov_y = self.fov_y.update(shot.fov_y, fov_half_life, dt);
        self.step_clearing(&frame, target.as_ref(), anchor, shot.pivot, fov_y, world, dt, snap);
        self.step_angles(&frame, target.as_ref(), anchor, fov_y, dt, snap);

        let across = self.pivot_across.update(
            Vec2::new(shot.pivot.x, shot.pivot.z),
            if snap { 0.0 } else { shot.pivot_half_life },
            dt,
        );
        let height =
            self.pivot_height.update(shot.pivot.y, if snap { 0.0 } else { shot.pivot_height_half_life }, dt);
        let pivot = Vec3::new(across.x, height, across.y);
        let yaw = self.yaw.follow(&self.tuning.look, dt, snap);
        let pitch = self.pitch.follow(&self.tuning.look, dt, snap);
        let axes = Axes::of(yaw, pitch);

        let goal = self.distance_goal(&frame, target.as_ref(), pivot, &axes, fov_y);
        let half_life = if snap {
            0.0
        } else if goal > self.distance.value {
            shot.pull_out_half_life
        } else {
            shot.push_in_half_life
        };
        let distance = self.distance.update(goal, half_life, dt);

        let eye_goal = pivot - axes.forward * distance;
        let eye =
            self.arm.resolve(world, anchor, eye_goal, &self.tuning.collision, dt, snap, &mut self.stats);

        let framed = target.as_ref().map(|target| target.at_height(self.tuning.explore.pivot_height));
        let framing = self.weight(Rig::Combat).max(self.weight(Rig::LockOn));
        let share = framed.map_or(0.0, |framed| shut_in(eye, anchor, framed) * framing);
        let share = self.shut_in.update(share, if snap { 0.0 } else { SHUT_IN_HALF_LIFE }, dt);
        let rotation = look_rotation(looking(eye, pivot, axes.forward, framed, share), &axes);
        let mut pose = CameraPose { eye, rotation, fov_y };

        let takeover = self.weight(Rig::Takeover);
        if takeover > 0.0
            && let Some(shot) = self.takeover.or(self.last_takeover)
        {
            pose = if takeover >= 1.0 {
                shot
            } else {
                CameraPose {
                    eye: pose.eye.lerp(shot.eye, takeover),
                    rotation: pose.rotation.slerp(shot.rotation, takeover),
                    fov_y: pose.fov_y + (shot.fov_y - pose.fov_y) * takeover,
                }
            };
        }

        let forward = pose.forward();
        if forward.x.abs().max(forward.z.abs()) > 1e-3 {
            self.heading = forward.x.atan2(forward.z);
        }

        self.shakes.advance(dt, frame.behind);
        let (turn, offset) = self.shakes.sample(&self.tuning.shakes);
        if offset != Vec3::ZERO {
            let moved = pose.rotation * Vec3::new(-offset.x, offset.y, offset.z);
            pose.eye = self.arm.offset_by(world, pose.eye, moved, &mut self.stats);
        }
        if turn != Vec3::ZERO {
            // Small angles: half of each, plus a scalar of 1, normalized.
            let small = Quat::from_xyzw(turn.x * 0.5, turn.y * 0.5, turn.z * 0.5, 1.0).normalize();
            pose.rotation = (pose.rotation * small).normalize();
        }

        self.yaw.rewrap();
        self.pose = pose;
        pose
    }

    /// `frame` with anything unusable replaced: NaNs from upstream shouldn't reach the springs, whose state
    /// would never recover. A paused or backward `dt` is 0.
    fn sanitized(&mut self, frame: &CameraFrame) -> CameraFrame {
        let mut clean = *frame;
        if !clean.dt.is_finite() || clean.dt < 0.0 {
            clean.dt = 0.0;
        }
        if !clean.behind.is_finite() || clean.behind < 0.0 {
            clean.behind = 0.0;
        }
        if !clean.look.is_finite() || clean.dt <= 0.0 {
            clean.look = Vec2::ZERO;
        }
        if !clean.zoom.is_finite() || clean.dt <= 0.0 {
            clean.zoom = 0.0;
        }
        if !clean.target.is_none_or(|target| target.is_valid()) {
            clean.target = None;
        }
        if let Some(last) = &self.last_frame {
            if !clean.player.is_valid() {
                clean.player = last.player;
            }
            if !clean.player_yaw.is_finite() {
                clean.player_yaw = last.player_yaw;
            }
            if !clean.aspect.is_finite() || clean.aspect <= 0.0 {
                clean.aspect = last.aspect;
            }
        } else {
            if !clean.player.is_valid() {
                clean.player = Subject { position: Vec3::ZERO, height: 1.0, radius: 0.1 };
            }
            if !clean.player_yaw.is_finite() {
                clean.player_yaw = 0.0;
            }
            if !clean.aspect.is_finite() || clean.aspect <= 0.0 {
                clean.aspect = 16.0 / 9.0;
            }
        }
        self.last_frame = Some(clean);
        clean
    }

    /// Moves each rig's weight toward on or off. Rigs that frame someone need someone to frame.
    fn step_weights(&mut self, frame: &CameraFrame, dt: f32, snap: bool) {
        let has_target = frame.target.is_some();
        let half_lives = [
            0.0,
            self.tuning.combat.framing.blend_half_life,
            self.tuning.lock_on.framing.blend_half_life,
            self.tuning.takeover.blend_half_life,
        ];
        for rig in [Rig::Combat, Rig::LockOn, Rig::Takeover] {
            let index = rig as usize;
            let usable = rig == Rig::Takeover || has_target;
            let goal = if self.wanted[index] && usable { 1.0 } else { 0.0 };
            if snap {
                self.weights[index] = goal;
                self.weight_speeds[index] = 0.0;
                continue;
            }
            // `damp`, not `Follow`: the goal steps, at the start of this frame.
            let weight =
                damp(self.weights[index], goal, &mut self.weight_speeds[index], half_lives[index], dt);
            let settled = (weight - goal).abs() < WEIGHT_SNAP && self.weight_speeds[index].abs() < 0.01;
            self.weights[index] = if settled { goal } else { weight.clamp(0.0, 1.0) };
            if settled {
                self.weight_speeds[index] = 0.0;
            }
        }
    }

    /// The target as the rigs see it: where it really is, except just after a change of target, when it
    /// comes from the old one's place. A rig fading out after the last one went keeps framing where it was.
    fn step_target(&mut self, frame: &CameraFrame, dt: f32, snap: bool) -> Option<Subject> {
        let half_life = if snap { 0.0 } else { self.tuning.look.target_half_life };
        let framing = self.weight(Rig::Combat) > 0.0 || self.weight(Rig::LockOn) > 0.0;
        match (frame.target, self.last_target) {
            (Some(target), Some(last)) if framing => {
                let reach = (TARGET_JUMP.0 * dt).max(TARGET_JUMP.1);
                if target.position.distance_squared(last.position) > reach * reach {
                    let was = last.position + self.target_offset.value;
                    self.target_offset.reset(was - target.position);
                    self.target_motion.reset(target.position);
                }
            }
            (Some(target), _) => {
                self.target_offset.reset(Vec3::ZERO);
                self.target_motion.reset(target.position);
            }
            (None, _) if framing => {}
            (None, _) => self.last_target = None,
        }
        self.target_offset.update(Vec3::ZERO, half_life, dt);
        self.last_target = frame.target.or(self.last_target);
        let subject = self.last_target?;
        self.target_motion.update(subject.position, VELOCITY_HALF_LIFE, dt);
        Some(Subject { position: subject.position + self.target_offset.value, ..subject })
    }

    /// Looks at whether the framing rigs' view is any good from here, and whether another would be
    /// better: the `clearing` module.
    #[allow(clippy::too_many_arguments, reason = "one frame's inputs, at its one call")]
    fn step_clearing(
        &mut self,
        frame: &CameraFrame,
        target: Option<&Subject>,
        anchor: Vec3,
        pivot: Vec3,
        fov_y: f32,
        world: &PhysicsWorld,
        dt: f32,
        snap: bool,
    ) {
        let (combat, lock_on) = (self.weight(Rig::Combat), self.weight(Rig::LockOn));
        let Some(target) = target.filter(|_| !snap && dt < LONG_FRAME && combat.max(lock_on) >= 0.5) else {
            self.clearing.reset();
            return;
        };
        let tuning = self.tuning.clearing;
        if frame.look != Vec2::ZERO {
            self.clearing.hand_over(&tuning);
        }
        let locked = lock_on >= 0.5;
        let framing = if locked { &self.tuning.lock_on.framing } else { &self.tuning.combat.framing };
        // Up close a line means little, so `line_fade` has lock-on let go, and the view's as free as combat's.
        let lead = target.position - frame.player.position;
        let gap = Vec2::new(lead.x, lead.z).length();
        let held = locked && smoothstep(framing.line_fade[0], framing.line_fade[1], gap) >= 0.5;
        let yaw = if held { self.lock_on_goal(frame, target) } else { self.yaw.heading };
        let scene = Scene {
            world,
            arm: &self.arm,
            anchor,
            player: &frame.player,
            target,
            pivot,
            pitch: self.pitch.heading,
            aspect: frame.aspect,
            half_fov: (fov_y * 0.5).tan(),
            framing,
            zoom: self.zoom.exp(),
            player_soon: frame.player.position + self.player_motion.velocity * LOOK_AHEAD,
            target_soon: target.position + self.target_motion.velocity * LOOK_AHEAD,
        };
        self.clearing.step(&scene, &tuning, yaw, held, dt, &mut self.stats);
    }

    /// The yaw lock-on holds, before any lean to clear what's in its way.
    fn lock_on_goal(&self, frame: &CameraFrame, target: &Subject) -> f32 {
        let lead = target.position - frame.player.position;
        lead.x.atan2(lead.z) + self.side * self.tuning.lock_on.side_angle + self.lean.x
    }

    /// Applies look input and the rigs' pulls to the yaw and pitch the view is heading for.
    fn step_angles(
        &mut self,
        frame: &CameraFrame,
        target: Option<&Subject>,
        anchor: Vec3,
        fov_y: f32,
        dt: f32,
        snap: bool,
    ) {
        let combat = self.weight(Rig::Combat);
        let lock_on = self.weight(Rig::LockOn);
        let explore = (1.0 - combat) * (1.0 - lock_on);
        let look_tuning = self.tuning.look;

        self.zoom = (self.zoom + frame.zoom).clamp(look_tuning.zoom_min, look_tuning.zoom_max);

        // Look input turns the view, and under lock-on it only leans it.
        self.yaw.heading += frame.look.x * (1.0 - lock_on);
        self.pitch.heading += frame.look.y * (1.0 - lock_on);
        self.step_lean(frame, lock_on, dt);
        if frame.look != Vec2::ZERO {
            self.recentering = false;
        }
        self.idle = if frame.look != Vec2::ZERO || explore < 1.0 { 0.0 } else { self.idle + dt };

        let mut yaw = Pull::default();
        let mut pitch = Pull::default();

        // Back behind the player: asked for, or drifting after a while with no input.
        if self.recentering {
            let rate = rate_of(look_tuning.recenter_half_life);
            let turn = wrap_angle(frame.player_yaw - self.yaw.heading);
            let tilt = self.tuning.explore.pitch - self.pitch.heading;
            yaw.add(turn, rate);
            pitch.add(tilt, rate);
            self.recentering = turn.abs().max(tilt.abs()) > RECENTERED;
        }
        let idle = smoothstep(look_tuning.idle_delay, look_tuning.idle_delay + 1.0, self.idle) * explore;
        if idle > 0.0 {
            let rate = rate_of(look_tuning.idle_half_life) * idle;
            let speed = Vec2::new(self.player_motion.velocity.x, self.player_motion.velocity.z).length();
            let moving = (speed / look_tuning.idle_full_speed).min(1.0);
            yaw.add(wrap_angle(frame.player_yaw - self.yaw.heading), rate * moving);
            pitch.add(self.tuning.explore.pitch - self.pitch.heading, rate);
        }

        if let Some(target) = target {
            let lead = target.position - frame.player.position;
            let gap = Vec2::new(lead.x, lead.z).length();
            let line = lead.x.atan2(lead.z);
            let off_line = wrap_angle(self.yaw.heading - line);
            if lock_on < 0.01 && off_line != 0.0 {
                // Which side lock-on will stand: the one the view's on already. Held while it's on.
                self.side = off_line.signum();
            }
            let fade = |framing: &FramingTuning| smoothstep(framing.line_fade[0], framing.line_fade[1], gap);
            let target_chest = target.at_height(self.tuning.explore.pivot_height);

            if combat > 0.0 {
                let framing = &self.tuning.combat.framing;
                let half_fov = (fov_y * 0.5).tan();
                let tan_x = half_fov * frame.aspect * framing.safe_frame[0];
                let height = framed_pivot(framing, anchor, target_chest).y;
                let above = (target.position.y + target.height - height).abs();
                let below = (target.position.y - height).abs();
                let depth = above.max(below) / (half_fov * framing.safe_frame[1]);
                let reach = gap - (gap * framing.pivot_bias).min(framing.max_lead);
                let room = self.zoom.exp() * framing.distance + framing.turn_slack;
                let limit = yaw_limit(tan_x, depth.max(framing.min_depth), room, reach);
                if off_line.abs() > limit {
                    let turn = off_line.signum() * limit - off_line;
                    yaw.add(turn, rate_of(framing.turn_half_life) * combat * fade(framing));
                }
                let (low, high) = (self.tuning.combat.pitch_min, self.tuning.combat.pitch_max);
                let inside = self.pitch.heading.clamp(low, high);
                if inside != self.pitch.heading {
                    pitch.add(inside - self.pitch.heading, rate_of(framing.turn_half_life) * combat);
                }
            }
            if let Some(goal) = self.clearing.goal() {
                let clearing = &self.tuning.clearing;
                // As much as a held line leaves free.
                let free = combat.max(lock_on) * (1.0 - lock_on * fade(&self.tuning.lock_on.framing));
                let top =
                    clearing.top_speed.min(clearing.top_eye_speed / self.distance.value.max(1.0)) / MOST_PULL;
                let rate = rate_of(clearing.half_life / self.clearing.haste()).min(top) * free;
                yaw.add(wrap_angle(goal - self.yaw.heading), rate);
            }
            if lock_on > 0.0 {
                let tuning = &self.tuning.lock_on;
                let rate = rate_of(tuning.framing.turn_half_life) * lock_on;
                let goal = line + self.side * tuning.side_angle + self.lean.x + self.clearing.lean();
                yaw.add(wrap_angle(goal - self.yaw.heading), rate * fade(&tuning.framing));

                let rise = target_chest.y - anchor.y;
                let tilt = tuning.pitch - tuning.pitch_follow * rise.atan2(gap.max(0.01)) + self.lean.y;
                pitch.add(tilt - self.pitch.heading, rate);
            }
        }

        self.yaw.steer(yaw, look_tuning.max_turn_rate, dt, snap);
        self.pitch.steer(pitch, look_tuning.max_turn_rate, dt, snap);
        self.pitch.heading = self.pitch.heading.clamp(look_tuning.pitch_min, look_tuning.pitch_max);
    }

    /// Lock-on's lean: input moves it within its limits, and it returns once input lets go.
    fn step_lean(&mut self, frame: &CameraFrame, lock_on: f32, dt: f32) {
        let tuning = &self.tuning.lock_on;
        let limits = Vec2::new(tuning.lean_yaw, tuning.lean_pitch);
        self.lean += frame.look * lock_on;
        self.lean = self.lean.clamp(-limits, limits);
        let decay = (-rate_of(tuning.lean_half_life) * dt).exp();
        if frame.look.x == 0.0 {
            self.lean.x *= decay;
        }
        if frame.look.y == 0.0 {
            self.lean.y *= decay;
        }
    }

    /// The stack's shot, blended by weight.
    fn shot(&self, anchor: Vec3, target: Option<&Subject>) -> Shot {
        let explore = &self.tuning.explore;
        let mut shot = Shot {
            pivot: anchor,
            fov_y: explore.fov_y,
            pivot_half_life: explore.pivot_half_life,
            pivot_height_half_life: explore.pivot_height_half_life,
            pull_out_half_life: explore.distance_half_life,
            push_in_half_life: explore.distance_half_life,
        };
        let Some(target) = target else { return shot };
        let target_at = target.at_height(self.tuning.explore.pivot_height);
        for (rig, framing) in
            [(Rig::Combat, &self.tuning.combat.framing), (Rig::LockOn, &self.tuning.lock_on.framing)]
        {
            let weight = self.weight(rig);
            if weight > 0.0 {
                shot = shot.mixed(&Shot::framing(framing, anchor, target_at), weight);
            }
        }
        shot
    }

    /// The distance the stack asks for. The framing rigs measure from the view as it stands this frame,
    /// so what they fit is what's drawn, and from where the fighters will be by the time the distance's
    /// spring catches up, so a fighter running at the camera is met rather than trailed.
    fn distance_goal(
        &self,
        frame: &CameraFrame,
        target: Option<&Subject>,
        pivot: Vec3,
        axes: &Axes,
        fov_y: f32,
    ) -> f32 {
        let zoom = self.zoom.exp();
        let mut goal = self.tuning.explore.distance * zoom;
        let Some(target) = target else { return goal };
        let half_fov = (fov_y * 0.5).tan();
        let pivot_speed =
            Vec3::new(self.pivot_across.velocity.x, self.pivot_height.velocity, self.pivot_across.velocity.y);
        for (rig, framing) in
            [(Rig::Combat, &self.tuning.combat.framing), (Rig::LockOn, &self.tuning.lock_on.framing)]
        {
            let weight = self.weight(rig);
            if weight > 0.0 {
                let (tan_x, tan_y) =
                    (half_fov * frame.aspect * framing.safe_frame[0], half_fov * framing.safe_frame[1]);
                let lead = framing.pull_out_half_life * LEAD_PER_HALF_LIFE;
                let soon = |subject: &Subject, motion: &Follow<Vec3>| Subject {
                    position: subject.position + (motion.velocity - pivot_speed) * lead,
                    ..*subject
                };
                let fitted =
                    framed_distance([&frame.player, target], pivot, axes, tan_x, tan_y, framing.min_depth)
                        .max(framed_distance(
                            [&soon(&frame.player, &self.player_motion), &soon(target, &self.target_motion)],
                            pivot,
                            axes,
                            tan_x,
                            tan_y,
                            framing.min_depth,
                        ))
                        .max(framing.distance * zoom)
                        .min(framing.max_distance);
                goal += (fitted - goal) * weight;
            }
        }
        goal
    }
}

/// What a rig asks of the shared springs.
#[derive(Clone, Copy, Debug)]
struct Shot {
    pivot: Vec3,
    fov_y: f32,
    pivot_half_life: f32,
    pivot_height_half_life: f32,
    pull_out_half_life: f32,
    push_in_half_life: f32,
}

impl Shot {
    fn framing(tuning: &FramingTuning, anchor: Vec3, target: Vec3) -> Self {
        Self {
            pivot: framed_pivot(tuning, anchor, target),
            fov_y: tuning.fov_y,
            pivot_half_life: tuning.pivot_half_life,
            pivot_height_half_life: tuning.pivot_height_half_life,
            pull_out_half_life: tuning.pull_out_half_life,
            push_in_half_life: tuning.push_in_half_life,
        }
    }

    fn mixed(&self, over: &Self, weight: f32) -> Self {
        let mix = |a: f32, b: f32| a + (b - a) * weight;
        Self {
            pivot: self.pivot.lerp(over.pivot, weight),
            fov_y: mix(self.fov_y, over.fov_y),
            pivot_half_life: mix(self.pivot_half_life, over.pivot_half_life),
            pivot_height_half_life: mix(self.pivot_height_half_life, over.pivot_height_half_life),
            pull_out_half_life: mix(self.pull_out_half_life, over.pull_out_half_life),
            push_in_half_life: mix(self.push_in_half_life, over.push_in_half_life),
        }
    }
}

/// The level rotation looking along `direction`. Near straight up or down, where it has no level, it
/// keeps `axes`' right.
fn look_rotation(direction: Vec3, axes: &Axes) -> Quat {
    let forward = direction.try_normalize().unwrap_or(axes.forward);
    let right = forward.cross(UP).try_normalize().unwrap_or(axes.right);
    let up = right.cross(forward);
    // Local +X is left: the engine's cameras face +Z.
    Quat::from_mat3(&Mat3::from_cols(-right, up, forward)).normalize()
}
