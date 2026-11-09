//! One tick of one character: velocity, a slide along what it hits, then what's under its feet.

use sapphire_math::{Quat, Vec2, Vec3, det};
use sapphire_physics::{
    BodyId, Group, HitTarget, MoveHit, MoveOptions, PhysicsWorld, Pose, QueryFilter, WorldError,
};
use sapphire_time::SimDuration;

use super::profile::ResolvedProfile;
use super::{
    ControllerDrive, ControllerEvents, ControllerInput, ControllerState, GroundMode, JUMP_REBOUND_WINDOW,
    MOVING_JUMP_MIN_ALIGNMENT, MOVING_JUMP_MIN_SPEED, MOVING_JUMP_PREPARATION, MovementMode,
    REBOUND_JUMP_PREPARATION, REBOUND_MIN_LANDING_SPEED, STANDING_JUMP_MAX_SPEED, STANDING_JUMP_PREPARATION,
};
use crate::movement::ActorTransform;

/// Moves one tick may make, each after turning along what the last one hit.
const MAX_SLIDES: usize = 4;
/// Motion under this is rounding, in metres.
const MIN_MOTION: f32 = 1e-6;
/// A gap this much over where the feet rest is rounding, not ground dropping away, in metres. It's
/// why a character standing still keeps its exact bits.
const SETTLE: f32 = 1e-4;
/// Skins of gap under the feet that count as touching at a landing.
const TOUCHING_SKINS: f32 = 4.0;
/// How far past the edge it hit a step-up looks for the top of the step, in metres.
const STEP_MARGIN: f32 = 0.02;
/// How far over the surface under its axis a step-up may leave the capsule, held up by something
/// else, in metres. It lets a stair's next edge bear a little of it, and refuses an edge the capsule
/// only rises to the corner of, with nothing under its axis yet.
const STEP_PERCH: f32 = 0.04;
/// A slide slower than this can jump: it's wedged, and would otherwise never get out, in m/s.
const WEDGED_SPEED: f32 = 0.1;
/// Upward speed under this is what's left of a landing's rounding, not a rise, in m/s.
const RISING: f32 = 0.01;
/// Dividing by a surface's upward part stops meaning anything below this.
const MIN_UPWARD: f32 = 0.05;

/// What a ray for ground can stop on: other characters aren't ground.
fn ground_filter() -> QueryFilter {
    QueryFilter::new(Group::World | Group::CharacterBlocker)
}

/// Runs one character for one tick of its owner's clock. A body that isn't in `world`, or isn't a
/// sphere or capsule, is an error.
///
/// **Frozen is `delta` zero:** nothing moves and nothing accumulates. Only last tick's
/// [`ControllerState::events`] clear, and a drive's motion is dropped, which a table can't notice:
/// its reading can't move while its clock stands. A jump asked for with the stick its own waits, and goes
/// on the first tick that moves, if the character can jump then and no move has taken the stick.
pub fn step_controller(
    world: &mut PhysicsWorld,
    body: BodyId,
    transform: &mut ActorTransform,
    state: &mut ControllerState,
    profile: &ResolvedProfile,
    input: &ControllerInput,
    delta: SimDuration,
) -> Result<(), WorldError> {
    let pose = world.body_pose(body).ok_or(WorldError::UnknownBody(body))?;
    let just_landed = state.events.landed;
    state.events = ControllerEvents::default();
    if delta == SimDuration::ZERO {
        state.jump_waiting |= input.jump && input.drive.locomotion;
        return Ok(());
    }
    let waited = std::mem::take(&mut state.jump_waiting) && input.drive.locomotion;
    let dt = delta.as_secs_f32();
    let drive = sanitized(&input.drive);
    let stick = if drive.locomotion && input.stick.is_finite() { input.stick } else { Vec2::ZERO };
    let want = profile.target_velocity(stick.clamp_length_max(1.0));
    let start = pose.position;
    let mut tick = Tick { world, body, profile, center: start, rotation: pose.rotation };

    let request = input.jump || waited;
    let jump = release_jump(state, request, just_landed, drive, want, delta);
    if jump && drive.gravity && can_jump(state, profile) {
        state.velocity.y = profile.authored().jump_speed;
        state.mode = GroundMode::Airborne;
        state.coyote = SimDuration::ZERO;
        state.events.jumped = true;
    }
    state.coyote = state.coyote - delta;

    // The average of a tick's two velocities is exact for constant acceleration, so a run start and
    // a jump's arc cover the same distance at any time scale.
    if !drive.gravity {
        state.velocity.y = 0.0;
    }
    if !drive.locomotion && state.mode != GroundMode::Sliding {
        // A move's table is all the motion, from its first tick, so what it moves by is exact.
        state.velocity = Vec3::new(0.0, state.velocity.y, 0.0);
    }
    let before = state.velocity;
    match state.mode {
        GroundMode::Grounded => ground_velocity(state, profile, want, drive.locomotion, dt),
        GroundMode::Sliding => slide_velocity(state, profile, want, drive.gravity, dt),
        GroundMode::Airborne => air_velocity(state, profile, want, drive.gravity, dt),
    }
    let travel = (before + state.velocity) * (0.5 * dt);
    let fall_speed = (-state.velocity.y).max(0.0);

    let root = drive.root_motion;
    if state.mode == GroundMode::Grounded && !drive.gravity && root.y > 0.0 {
        state.mode = GroundMode::Airborne;
        state.events.left_ground = true;
    }
    let motion = match state.mode {
        // While grounded with gravity on, the floor decides the height, a table's included.
        GroundMode::Grounded => {
            let planar = flat(travel + root);
            along_ground(planar, planar.length(), state.ground_normal)
        }
        GroundMode::Sliding | GroundMode::Airborne => travel + root,
    };

    tick.slide(state, motion, drive.mode == MovementMode::Free)?;
    tick.settle(state, &drive, start, fall_speed)?;
    if state.jump_preparation.is_some() {
        state.jump_rebound = if state.events.landed && state.events.landing_speed > REBOUND_MIN_LANDING_SPEED
        {
            JUMP_REBOUND_WINDOW
        } else if state.mode == GroundMode::Grounded {
            state.jump_rebound - delta
        } else {
            SimDuration::ZERO
        };
    }

    let facing = drive.face.or_else(|| (want != Vec3::ZERO).then(|| det::yaw_of_direction(want)));
    if let Some(target) = facing {
        transform.yaw = det::turn_towards(transform.yaw, target, profile.turn_rate() * dt);
    }
    let rotation = det::quat_from_rotation_y(transform.yaw);
    if rotation != tick.rotation {
        tick.rotation = rotation;
        tick.place(tick.center)?;
    }
    transform.position = tick.center - profile.feet_to_center();
    Ok(())
}

/// A drive with anything that can't be applied made safe: the world refuses non-finite motion, and
/// one bad table mustn't stop a whole step.
fn sanitized(drive: &ControllerDrive) -> ControllerDrive {
    let root_motion = if drive.root_motion.is_finite() { drive.root_motion } else { Vec3::ZERO };
    let face = drive.face.filter(|face| face.is_finite());
    ControllerDrive { root_motion, face, ..*drive }
}

/// A supported press can load the soles. Turning away releases a pending press, while a move
/// taking the stick cancels it. Coyote and move-authored jumps still use the ordinary path.
fn release_jump(
    state: &mut ControllerState,
    request: bool,
    just_landed: bool,
    drive: ControllerDrive,
    want: Vec3,
    delta: SimDuration,
) -> bool {
    let Some(remaining) = &mut state.jump_preparation else { return request };
    if !drive.locomotion
        || !drive.gravity
        || drive.mode != MovementMode::Free
        || drive.root_motion != Vec3::ZERO
    {
        *remaining = SimDuration::ZERO;
        state.jump_preparation_moving = false;
        return request;
    }
    let velocity = flat(state.velocity);
    let speed = velocity.length();
    let aligned = speed >= MOVING_JUMP_MIN_SPEED
        && velocity.dot(want) >= MOVING_JUMP_MIN_ALIGNMENT * speed * want.length()
        && want != Vec3::ZERO;
    if *remaining != SimDuration::ZERO {
        let follows_plant = if state.jump_preparation_moving { aligned } else { want == Vec3::ZERO };
        *remaining = if follows_plant && state.mode == GroundMode::Grounded && !drive.jump {
            *remaining - delta
        } else {
            SimDuration::ZERO
        };
        if *remaining == SimDuration::ZERO {
            state.jump_preparation_moving = false;
            return true;
        }
        return false;
    }
    if !request || drive.jump || just_landed || state.mode != GroundMode::Grounded {
        return request;
    }
    if speed <= STANDING_JUMP_MAX_SPEED && want == Vec3::ZERO {
        *remaining = if state.jump_rebound > SimDuration::ZERO {
            REBOUND_JUMP_PREPARATION
        } else {
            STANDING_JUMP_PREPARATION
        };
        return false;
    }
    if aligned {
        *remaining = MOVING_JUMP_PREPARATION;
        state.jump_preparation_moving = true;
        return false;
    }
    true
}

/// Whether a press jumps from where it is: from the ground, off a ledge just left, out of a wedge, and from
/// the one slide gentle enough to walk, on another character's top ([`slide_velocity`]), where a stick has it
/// walking and a press can't wait for it to stop.
fn can_jump(state: &ControllerState, profile: &ResolvedProfile) -> bool {
    match state.mode {
        GroundMode::Grounded => true,
        GroundMode::Airborne => state.coyote > SimDuration::ZERO,
        GroundMode::Sliding => {
            state.velocity.length() < WEDGED_SPEED || profile.walkable(state.ground_normal)
        }
    }
}

fn ground_velocity(
    state: &mut ControllerState,
    profile: &ResolvedProfile,
    want: Vec3,
    locomotion: bool,
    dt: f32,
) {
    if !locomotion {
        return;
    }
    let p = profile.authored();
    let velocity = flat(state.velocity);
    let rate = if velocity.dot(want) < 0.0 {
        p.acceleration.max(p.deceleration)
    } else if want.length_squared() < velocity.length_squared() {
        p.deceleration
    } else {
        p.acceleration
    };
    state.velocity = move_towards(velocity, want, rate * dt);
}

fn air_velocity(state: &mut ControllerState, profile: &ResolvedProfile, want: Vec3, gravity: bool, dt: f32) {
    let p = profile.authored();
    let mut planar = flat(state.velocity);
    if want != Vec3::ZERO {
        planar = move_towards(planar, want, p.air_acceleration * dt);
    }
    let fall = if gravity { (state.velocity.y - p.gravity * dt).max(-p.max_fall_speed) } else { 0.0 };
    state.velocity = Vec3::new(planar.x, fall, planar.z);
}

/// Gravity along the slope, and the stick steering across it: never up it, or a strong enough stick
/// would climb what's too steep to stand on.
///
/// The one slide gentle enough to stand on is on top of another character, which is never ground. There
/// nothing is too steep, and gravity has next to no downhill to go by, and at the very top none at all,
/// so the stick walks it off whichever way it points. Left alone dead center, it stays until a jump or
/// the one under it moves.
fn slide_velocity(
    state: &mut ControllerState,
    profile: &ResolvedProfile,
    want: Vec3,
    gravity: bool,
    dt: f32,
) {
    let p = profile.authored();
    let normal = state.ground_normal;
    let mut velocity = state.velocity;
    if gravity {
        velocity.y -= p.gravity * dt;
    }
    if want != Vec3::ZERO && profile.walkable(normal) {
        let planar = move_towards(flat(velocity), want, p.acceleration * dt);
        velocity = Vec3::new(planar.x, velocity.y, planar.z);
    } else if let Some(downhill) = horizontal(normal) {
        let across = Vec3::new(-downhill.z, 0.0, downhill.x);
        let now = velocity.dot(across);
        let next = move_towards_scalar(now, want.dot(across), p.air_acceleration * dt);
        velocity += across * (next - now);
    }
    velocity -= normal * velocity.dot(normal).min(0.0);
    state.velocity = velocity.clamp_length_max(p.max_slide_speed);
}

struct Tick<'a> {
    world: &'a mut PhysicsWorld,
    body: BodyId,
    profile: &'a ResolvedProfile,
    center: Vec3,
    rotation: Quat,
}

impl Tick<'_> {
    fn place(&mut self, center: Vec3) -> Result<(), WorldError> {
        self.center = center;
        self.world.set_body_pose(self.body, Pose::new(center, self.rotation))
    }

    /// Moves by `motion`, turning along what it meets.
    ///
    /// Each move makes one sweep rather than leaving the sliding to `move_and_collide`, whose slide
    /// would carry a character up any slope it meets. A move with no motion still resolves any
    /// overlap (a spawn inside a wall), so only a character standing on the ground skips it.
    fn slide(
        &mut self,
        state: &mut ControllerState,
        mut motion: Vec3,
        may_step: bool,
    ) -> Result<(), WorldError> {
        let walking = state.mode == GroundMode::Grounded;
        if walking && motion.length_squared() < MIN_MOTION * MIN_MOTION {
            return Ok(());
        }
        let options = MoveOptions { skin: self.profile.authored().skin, max_sweeps: 1 };
        let mut planes = Planes::default();
        if walking {
            planes.push(state.ground_normal);
        }
        // A walker's velocity is horizontal, so its surfaces don't include the ground.
        let mut velocity_planes = Planes::default();
        let mut stepped = false;

        for _ in 0..MAX_SLIDES {
            let outcome = self.world.move_and_collide(self.body, motion, &options)?;
            self.center = outcome.position;
            state.events.pushed_out |= outcome.started_overlapping();
            let Some(hit) = outcome.hits().first().copied() else { break };
            motion = outcome.remaining;

            if !walking {
                // What it meets never lifts someone in the air: turned along a curb's edge or a steep
                // face, the speed going across would otherwise go up, and throw them higher than a jump
                // reaches (M3 B4). It can still stop a rise, or a fall.
                state.velocity =
                    no_lift(velocity_planes.clip(state.velocity, hit.normal), state.velocity, RISING);
                motion = no_lift(planes.clip(motion, hit.normal), motion, SETTLE);
            } else if is_static(hit.target) && self.profile.walkable(hit.normal) {
                state.ground_normal = hit.normal;
                planes = Planes::default();
                planes.push(hit.normal);
                motion = along_ground(flat(motion), motion.length(), hit.normal);
            } else if may_step && !stepped && self.step_up(state, &hit, motion)? {
                stepped = true;
                planes = Planes::default();
                planes.push(state.ground_normal);
                motion = along_ground(flat(motion), motion.length(), state.ground_normal);
            } else {
                // A steep slope stops a walker as a wall would, or the slide would carry it up.
                let wall = horizontal(hit.normal).unwrap_or(hit.normal);
                state.velocity = flat(velocity_planes.clip(state.velocity, wall));
                motion = planes.clip(motion, if hit.normal.y > 0.0 { wall } else { hit.normal });
            }
            if motion.length_squared() < MIN_MOTION * MIN_MOTION {
                break;
            }
        }
        Ok(())
    }

    /// Tries to take the step in front of a walker: room overhead, room over the step, then ground to
    /// stand on that's no higher than a step. All of it is casts, so a failure moves nothing.
    ///
    /// It lifts the capsule to the top so the tick's motion can carry it on, and [`Self::settle`] then
    /// lowers it onto whatever its round foot meets, which is the step's edge until its axis is over
    /// the step. So a step reads as a short steep ramp over the edge, a few ticks at a run, rather than a
    /// pop, and a character that stops partway stands on the edge, in reach of the floor it came from.
    fn step_up(
        &mut self,
        state: &mut ControllerState,
        hit: &MoveHit,
        motion: Vec3,
    ) -> Result<bool, WorldError> {
        let p = self.profile.authored();
        if !is_static(hit.target) {
            return Ok(false);
        }
        let Some(into) = horizontal(-hit.normal) else { return Ok(false) };
        if motion.dot(into) <= 0.0 {
            return Ok(false);
        }
        let want = p.step_height + 2.0 * p.skin;
        let rise = match self.world.cast_body(self.body, Vec3::ZERO, Vec3::Y, want)? {
            Some(ceiling) => ceiling.distance - p.skin,
            None => want,
        };
        if rise <= 0.0 {
            return Ok(false);
        }
        let lift = Vec3::Y * rise;
        // Just far enough to stand over what it hit: any further might find the next step's edge.
        let reach = (hit.point - self.center).dot(into).max(0.0) + STEP_MARGIN;
        let across = match self.world.cast_body(self.body, lift, into, reach)? {
            Some(ahead) => ahead.distance - p.skin,
            None => reach,
        };
        if across <= p.skin {
            return Ok(false);
        }
        let over = lift + into * across;
        let Some(top) = self.world.cast_body(self.body, over, Vec3::NEG_Y, rise + 2.0 * p.skin)? else {
            return Ok(false);
        };
        if !is_static(top.target) || top.started_overlapping {
            return Ok(false);
        }
        // What the axis stands over, which isn't always what the capsule touches first: a stair's
        // tread can be narrower than the capsule, so its round foot meets the next stair's edge.
        let foot = self.center + over - Vec3::Y * self.profile.half_height();
        let ground = self.world.cast_ray(foot, Vec3::NEG_Y, p.radius + rise + 2.0 * p.skin, &ground_filter());
        let Some(ground) = ground.filter(|ground| self.profile.walkable(ground.normal)) else {
            return Ok(false);
        };
        let surface = foot.y - ground.distance;
        let feet = self.center.y - self.profile.feet_to_center().y;
        let up = rise - top.distance + p.skin / top.normal.y.max(MIN_UPWARD);
        // Where the feet would rest on that surface by itself: on a slope, over it by the round foot.
        let reach = p.radius + p.skin;
        let rests_at = surface + reach / ground.normal.y - reach;
        if up <= SETTLE || surface - feet > p.step_height || feet + up - rests_at > STEP_PERCH {
            return Ok(false);
        }
        self.place(self.center + Vec3::Y * up)?;
        state.ground_normal = ground.normal;
        state.events.stepped_up = true;
        state.events.height_pop += up;
        Ok(true)
    }

    /// What's under the feet now, within `snap` of where they'd rest, plus what `travelled` metres
    /// would have dropped on ground as steep as what it finds: so a run over the top of a slope keeps
    /// to it, and flat ground far below is still a fall, however fast.
    ///
    /// A capsule's contact normal tilts as its round foot goes over an edge, so a ledge's edge reads
    /// like a slope. A contact near the axis is good either way. Past that, a ray down the axis tells
    /// a slope (it finds the same surface) from an edge (it finds nothing, or a floor further down),
    /// and flat ground and gentle slopes never pay for the ray.
    fn footing(&self, snap: f32, travelled: f32) -> Result<Footing, WorldError> {
        let p = self.profile.authored();
        let reach = |normal: Vec3| {
            let slope = flat(normal).length() / normal.y.max(MIN_UPWARD);
            snap + travelled * slope + TOUCHING_SKINS * p.skin
        };
        let cast = snap + travelled * self.profile.walkable_tan() + TOUCHING_SKINS * p.skin;
        let Some(hit) = self.world.cast_body(self.body, Vec3::ZERO, Vec3::NEG_Y, cast)? else {
            return Ok(Footing::Nothing);
        };
        if !is_static(hit.target) {
            return Ok(Footing::Steep { normal: hit.normal });
        }
        let drop = hit.distance - p.skin / hit.normal.y.max(MIN_UPWARD);
        let off_axis = flat(hit.point - self.center).length();
        if self.profile.walkable(hit.normal) && off_axis <= p.stance_radius {
            return Ok(if hit.distance <= reach(hit.normal) {
                Footing::Ground { normal: hit.normal, drop }
            } else {
                Footing::Nothing
            });
        }
        let foot = self.center - Vec3::Y * self.profile.half_height();
        let floor_reach = (p.radius + cast) / self.profile.walkable_cos();
        let steep_reach = (p.radius + 2.0 * p.skin) / hit.normal.y.max(MIN_UPWARD);
        let ray = self.world.cast_ray(foot, Vec3::NEG_Y, floor_reach.max(steep_reach), &ground_filter());
        Ok(match ray {
            Some(ray) if self.profile.walkable(ray.normal) => {
                if ray.distance <= (p.radius + reach(ray.normal)) / ray.normal.y {
                    Footing::Ground { normal: ray.normal, drop }
                } else {
                    Footing::Nothing
                }
            }
            Some(ray) if ray.normal.y > 0.0 => Footing::Steep { normal: ray.normal },
            _ => Footing::Nothing,
        })
    }

    /// Decides the mode from what's under the feet, and lowers them onto the ground.
    fn settle(
        &mut self,
        state: &mut ControllerState,
        drive: &ControllerDrive,
        start: Vec3,
        fall_speed: f32,
    ) -> Result<(), WorldError> {
        let p = self.profile.authored();
        if state.mode == GroundMode::Grounded {
            state.velocity.y = 0.0;
            let travelled = flat(self.center - start).length();
            let (snap, followed) = if drive.gravity { (p.ground_snap, travelled) } else { (0.0, 0.0) };
            match self.footing(snap, followed)? {
                Footing::Ground { normal, drop } => {
                    state.ground_normal = normal;
                    self.rest(state, drop)?;
                }
                Footing::Steep { .. } | Footing::Nothing
                    if drive.mode == MovementMode::Combat && travelled > 0.0 =>
                {
                    // A strike stays on the ledge it started on, rather than leaving it mid-swing.
                    self.place(start)?;
                    state.velocity = Vec3::ZERO;
                    state.events.ledge_held = true;
                }
                Footing::Steep { normal } => {
                    state.mode = GroundMode::Sliding;
                    state.ground_normal = normal;
                    state.events.left_ground = true;
                }
                Footing::Nothing => {
                    state.mode = GroundMode::Airborne;
                    state.coyote = SimDuration::from_ticks(u64::from(p.coyote_ticks));
                    state.events.left_ground = true;
                }
            }
            return Ok(());
        }
        if state.velocity.y > RISING || drive.root_motion.y > 0.0 {
            state.mode = GroundMode::Airborne;
            return Ok(());
        }
        match self.footing(0.0, 0.0)? {
            Footing::Ground { normal, drop } => {
                state.mode = GroundMode::Grounded;
                state.ground_normal = normal;
                state.velocity.y = 0.0;
                state.coyote = SimDuration::ZERO;
                state.events.landed = true;
                state.events.landing_speed = fall_speed;
                self.rest(state, drop)?;
            }
            Footing::Steep { normal } => {
                state.mode = GroundMode::Sliding;
                state.ground_normal = normal;
            }
            Footing::Nothing => state.mode = GroundMode::Airborne,
        }
        Ok(())
    }

    /// Moves the feet by `drop` onto the ground. Down has been cast, so it's clear. Up happens when a
    /// shallow approach left less than the skin, which a sweep doesn't hold, and it stops for a
    /// ceiling. Within [`SETTLE`] nothing moves.
    fn rest(&mut self, state: &mut ControllerState, drop: f32) -> Result<(), WorldError> {
        if drop > SETTLE {
            self.place(self.center - Vec3::Y * drop)?;
            state.events.height_pop -= drop;
        } else if drop < -SETTLE {
            let options = MoveOptions { skin: self.profile.authored().skin, max_sweeps: 1 };
            let raised = self.world.move_and_collide(self.body, Vec3::Y * -drop, &options)?;
            self.center = raised.position;
            state.events.height_pop += raised.moved.y;
        }
        Ok(())
    }
}

enum Footing {
    /// Nothing to stand on in reach.
    Nothing,
    /// Too steep to stand on, or another character.
    Steep { normal: Vec3 },
    /// Ground: its surface's normal, and how far down the feet rest on it.
    Ground { normal: Vec3, drop: f32 },
}

/// The surfaces a tick has slid along, so that sliding along one can't run back into another.
#[derive(Default)]
struct Planes {
    normals: [Vec3; MAX_SLIDES + 1],
    len: usize,
}

impl Planes {
    /// How far into a plane counts as rounding, in metres.
    const INTO: f32 = -1e-7;

    fn push(&mut self, normal: Vec3) {
        if self.len < self.normals.len() {
            self.normals[self.len] = normal;
            self.len += 1;
        }
    }

    /// What's left of `motion` after meeting a surface facing `normal`: along it, or in the crease
    /// with an earlier surface if that would run back into one, or nothing if the crease does too.
    /// This is what keeps a corner from jittering.
    fn clip(&mut self, motion: Vec3, normal: Vec3) -> Vec3 {
        let along = motion - normal * motion.dot(normal).min(0.0);
        let earlier = self.normals;
        let earlier = &earlier[..self.len];
        self.push(normal);
        let Some(blocking) = earlier.iter().find(|plane| along.dot(**plane) < Self::INTO) else {
            return along;
        };
        let Some(crease) = blocking.cross(normal).try_normalize() else { return Vec3::ZERO };
        let creased = crease * along.dot(crease);
        if earlier.iter().any(|plane| creased.dot(*plane) < Self::INTO) {
            return Vec3::ZERO;
        }
        creased
    }
}

/// `turned`, rising no faster than `before` did, or not at all if `before` wasn't rising. Within
/// `rounding` of that it's left alone, as what a floor's normal a hair off vertical leaves, not a lift.
fn no_lift(turned: Vec3, before: Vec3, rounding: f32) -> Vec3 {
    let most = before.y.max(0.0);
    if turned.y > most + rounding { Vec3::new(turned.x, most, turned.z) } else { turned }
}

fn is_static(target: HitTarget) -> bool {
    matches!(target, HitTarget::Static(_))
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// A vector's horizontal part as a unit vector, if it has one.
fn horizontal(v: Vec3) -> Option<Vec3> {
    let flat = flat(v);
    if flat.length_squared() < 1e-8 { None } else { flat.try_normalize() }
}

/// Horizontal motion laid onto ground facing `normal`, keeping its heading and making its length
/// `length`, so authored speeds are speeds over the ground, uphill or down.
fn along_ground(motion: Vec3, length: f32, normal: Vec3) -> Vec3 {
    let on_plane = Vec3::new(motion.x, -(normal.x * motion.x + normal.z * motion.z) / normal.y, motion.z);
    let plane_length = on_plane.length();
    if plane_length > 0.0 { on_plane * (length / plane_length) } else { Vec3::ZERO }
}

fn move_towards(from: Vec3, to: Vec3, max_step: f32) -> Vec3 {
    let between = to - from;
    let distance = between.length();
    if distance <= max_step { to } else { from + between * (max_step / distance) }
}

fn move_towards_scalar(from: f32, to: f32, max_step: f32) -> f32 {
    from + (to - from).clamp(-max_step, max_step)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slide_keeps_what_runs_along_the_surface() {
        let mut planes = Planes::default();
        assert_eq!(planes.clip(Vec3::new(3.0, 0.0, -4.0), Vec3::Z), Vec3::new(3.0, 0.0, 0.0));
    }

    /// Into a corner of walls facing +x and +z: along the first would run back into the second, so
    /// only their crease is left, which is straight up, and nothing here moves along it.
    #[test]
    fn a_corner_stops_a_slide() {
        let mut planes = Planes::default();
        let along = planes.clip(Vec3::new(-1.0, 0.0, -2.0), Vec3::Z);
        assert_eq!(along, Vec3::new(-1.0, 0.0, 0.0));
        assert_eq!(planes.clip(along, Vec3::X), Vec3::ZERO);
    }

    #[test]
    fn opposite_walls_stop_a_slide_that_runs_back() {
        let mut planes = Planes::default();
        planes.push(Vec3::NEG_X);
        assert_eq!(planes.clip(Vec3::new(1.0, 0.0, 0.0), Vec3::X), Vec3::ZERO);
    }

    /// On a floor, between two walls that meet: a slide along the first runs into the second.
    #[test]
    fn the_floor_and_two_walls_stop_a_slide() {
        let mut planes = Planes::default();
        planes.push(Vec3::Y);
        let along = planes.clip(Vec3::new(1.0, 0.0, 1.0), Vec3::NEG_X);
        assert_eq!(along, Vec3::new(0.0, 0.0, 1.0));
        assert_eq!(planes.clip(along, Vec3::NEG_Z), Vec3::ZERO);
    }

    #[test]
    fn ground_motion_keeps_its_length_and_heading() {
        // 30° up along +x: the normal leans back along -x, and a metre over the ground rises by the
        // slope's sine and covers its cosine.
        let (sin, cos) = det::sin_cos(30.0_f32.to_radians());
        let on_slope = along_ground(Vec3::X, 1.0, Vec3::new(-sin, cos, 0.0));
        assert!((on_slope - Vec3::new(cos, sin, 0.0)).length() < 1e-6, "{on_slope}");
    }

    #[test]
    fn moving_towards_lands_exactly_on_the_target() {
        assert_eq!(move_towards(Vec3::ZERO, Vec3::X, 0.25), Vec3::new(0.25, 0.0, 0.0));
        assert_eq!(move_towards(Vec3::new(0.9, 0.0, 0.0), Vec3::X, 0.25), Vec3::X);
        assert_eq!(move_towards_scalar(-1.0, 1.0, 0.5), -0.5);
    }
}
