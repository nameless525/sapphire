//! A tick on a climbable face (M6 A16a): what the controller does while [`ControllerState::climb`] holds one.
//!
//! # Where it holds
//!
//! The capsule hangs off the face by [`hold_gap`] past its radius (two of the profile's skins), along the
//! face's normal, which is the normal of the face's own triangle (or box side) where the hands' ray last met it,
//! from the colliders. The hands are the capsule's top ([`hands`]): a face is held where they are, so a climb's
//! top is where they come over the face's lip, and a tagged face ends where they find no more of it.
//!
//! # What the stick does
//!
//! The stick is read against the face, with the camera's handedness ([`read`]): its part toward the face (up
//! on the stick, with the camera behind the climber) climbs up the face, its part away climbs down, and its
//! sides go along the face to the left and right as the camera sees them, all in the face's plane at the
//! profile's `climb_speed`, colliding as any move does. Across a corner turn the stick's meaning latches
//! ([`ClimbHold::frame`], [`ClimbHold::latched`]) until it's released or swings more than 45°: a stick held
//! sideways carries round an outside corner and on along an inside one, and never turns into a climb down or
//! a push back into the face it left. Each tick that moves:
//!
//! 1. **The move:** one sweep. If it meets walkable ground on the way down, the climber lets go onto it.
//! 2. **An inside corner:** if it ran sideways into something across its way, a ray along the way it went looks
//!    for a climbable face there, within 45° of facing it ([`super::super::traversal::APPROACH_COS`]), and the
//!    climber turns onto it.
//! 3. **The hold:** a ray from the hands into the face, `radius + climb_reach` long, must find a climbable face
//!    within 45° of the one held. The climber takes its normal (so it follows a face that bends gently) and is
//!    swept to the gap off it when it's further than rounding from there.
//! 4. **An outside corner:** with no face for the hands after going sideways, a ray from just inside where the
//!    face was, back along the way it went, looks for the face around the corner; the capsule goes round it in
//!    two sweeps (along, then in), a ray confirms the face there and a sweep settles it to the gap.
//! 5. **A diagonal past a side end:** with nothing held after a move both up and sideways, the upward part is
//!    tried alone, so brushing a tagged face's side end on the way up still climbs.
//! 6. **Otherwise it stays where it was:** past the tagged face's end, or, when the upward part alone found no
//!    face, at its top: the hands have reached the lip ([`Lip::Reached`]), and from there the stick takes it no
//!    higher. A top-out is a move, which [`crate::traversal`] starts from there; a refused one
//!    ([`Lip::Refused`]) is tried again once the climber has moved along the top.
//!
//! A tick hanging still casts the hold's ray, 1 (and a sweep to the gap on a grab's first tick); a tick moving
//! on a flat face 2, the sweep and the hold, and 3 when it settles back to the gap; an inside corner 3, the
//! sweep, the corner's ray and the settle; an outside corner up to 7, the sweep, the hold that misses, the
//! corner's ray, two sweeps round, the ray that holds it there and the settle; a diagonal past a side end 3
//! more for its upward part. A tick that tries all of them, a corner ray that misses, a corner round that
//! misses and the upward part, is at most 10. Only static scenery in [`Group::Climbable`] is held: a face in
//! [`Group::World`] alone is never climbed, and the move's sweep meets whatever the capsule meets.
//!
//! # Letting go
//!
//! [`step`] returns `false` when the climber lets go this tick: its feet came down on ground, or its stamina
//! ran out. [`ControllerState::stamina_spent`] counts the owner's time on a face; it comes back whole only when
//! the feet next stand on the ground (there is no rate of recovery yet), and with the profile's stamina off it
//! never counts. It's the fighter's own: a party's swap neither hands it on nor takes it away. A press of jump,
//! and a move taking the stick, let go as well, which the caller does.

use sapphire_math::{Vec2, Vec3, det};
use sapphire_physics::{
    BodyId, Group, HitTarget, MoveOptions, PhysicsWorld, Pose, QueryFilter, RayHit, WorldError,
};
use sapphire_time::SimDuration;

use super::profile::ResolvedProfile;
use super::{ClimbHold, ControllerState, GroundMode, Lip, TraversalMode};
use crate::movement::ActorTransform;
use crate::traversal::APPROACH_COS;

/// How far inside a face's plane the outside corner's ray looks back from, in metres.
const CORNER_IN: f32 = 0.1;
/// A face whose normal points further down than this is a ceiling, never climbed: about 30° past vertical.
const OVERHANG: f32 = -0.5;
/// Motion under this is rounding, in metres, as the ground's.
const MIN_MOTION: f32 = 1e-6;
/// A gap off the face this close to [`hold_gap`] is held as it is, in metres.
const SETTLE: f32 = 1e-4;
/// A stick shorter than this is released: it moves nothing and lets go of a latched meaning.
const RELEASED: f32 = 1e-3;

/// How far past its radius the capsule hangs off a held face, in metres: two of the profile's skins, so a sweep
/// along the face, which stops a skin short, never touches it.
fn hold_gap(profile: &ResolvedProfile) -> f32 {
    2.0 * profile.authored().skin
}

/// What a climb's rays meet: tagged scenery, never a character.
fn climb_filter() -> QueryFilter {
    QueryFilter::new(Group::Climbable.mask())
}

/// Where a capsule centred at `center` holds a face: its top.
pub(crate) fn hands(profile: &ResolvedProfile, center: Vec3) -> Vec3 {
    center + Vec3::Y * (profile.half_height() + profile.authored().radius)
}

/// A climbable face along a ray from `from`, if it meets one a climber can hold: static, steep enough not to
/// stand on, and not a ceiling.
pub(crate) fn face(
    world: &PhysicsWorld,
    profile: &ResolvedProfile,
    from: Vec3,
    direction: Vec3,
    reach: f32,
) -> Option<RayHit> {
    world.cast_ray(from, direction, reach, &climb_filter()).filter(|hit| {
        matches!(hit.target, HitTarget::Static(_))
            && hit.distance > 0.0
            && !profile.walkable(hit.normal)
            && hit.normal.y >= OVERHANG
            && horizontal(hit.normal).is_some()
    })
}

/// Holds `normal`'s face: what a press in front of one does to the state (M6 A16a). The character hangs where it
/// is, nothing it was doing in the air or toward a jump carried, until its first tick on the face.
pub(crate) fn grab(state: &mut ControllerState, normal: Vec3) {
    state.traversal = TraversalMode::Climbing;
    state.climb = Some(ClimbHold::on(normal));
    state.mode = GroundMode::Airborne;
    state.velocity = Vec3::ZERO;
    state.coyote = SimDuration::ZERO;
    state.jump_waiting = false;
    if let Some(remaining) = &mut state.jump_preparation {
        *remaining = SimDuration::ZERO;
    }
    state.jump_preparation_moving = false;
}

/// Lets go of a face, in the air from here, with no speed of its own: it falls. What its stamina spent stays.
pub(crate) fn let_go(state: &mut ControllerState) {
    state.traversal = TraversalMode::None;
    state.climb = None;
    state.mode = GroundMode::Airborne;
    state.velocity = Vec3::ZERO;
    state.coyote = SimDuration::ZERO;
}

/// The unit direction into a face, level, from its normal.
pub(crate) fn into_face(normal: Vec3) -> Vec3 {
    horizontal(-normal).unwrap_or(Vec3::NEG_Z)
}

/// The face's plane's axes: up it, and along it to the climber's right as it faces it.
fn axes(normal: Vec3) -> (Vec3, Vec3) {
    let side = Vec3::Y.cross(normal).try_normalize().unwrap_or(Vec3::X);
    (normal.cross(side), side)
}

/// How the stick reads on a hold's face.
#[derive(Clone, Copy, Debug)]
struct Reading {
    /// The way it climbs, in the held face's plane and at most a unit long.
    way: Vec3,
    /// The face it was read against, and the latch it was read with: see [`ClimbHold::frame`].
    frame: Vec3,
    latched: Vec2,
    /// The stick's own direction, a unit long, or zero when it's released.
    direction: Vec2,
}

/// Reads the stick, which is in the world's plane as for walking, against `hold`'s face: its part into the
/// frame's face is up and its part along it to the right is right, applied to the held face's own axes. The
/// frame is the held face, or the face a corner turn latched while the stick still points within 45° of how it
/// pointed then.
fn read(stick: Vec2, hold: &ClimbHold) -> Reading {
    let stick = if stick.is_finite() { stick.clamp_length_max(1.0) } else { Vec2::ZERO };
    let length = stick.length();
    if length < RELEASED {
        let (frame, latched, direction) = (hold.normal, Vec2::ZERO, Vec2::ZERO);
        return Reading { way: Vec3::ZERO, frame, latched, direction };
    }
    let direction = stick / length;
    let kept = hold.latched != Vec2::ZERO && direction.dot(hold.latched) >= APPROACH_COS;
    let (frame, latched) = if kept { (hold.frame, hold.latched) } else { (hold.normal, Vec2::ZERO) };
    let flat = Vec3::new(stick.x, 0.0, stick.y);
    let (_, frame_side) = axes(frame);
    let (right, rise) = (flat.dot(frame_side), flat.dot(into_face(frame)));
    let (up, side) = axes(hold.normal);
    Reading { way: (up * rise + side * right).clamp_length_max(1.0), frame, latched, direction }
}

/// Whether the stick asks a climber holding `hold` to go higher.
pub(crate) fn wants_up(stick: Vec2, hold: &ClimbHold) -> bool {
    read(stick, hold).way.y > 0.0
}

/// Where a move on the face took the climber.
enum Went {
    /// Its feet came down on walkable ground.
    Ground,
    /// It holds a face at `at`, `normal`'s, having turned a corner onto it if `cornered`.
    Held { at: Vec3, normal: Vec3, cornered: bool },
    /// It found nothing to hold.
    Missed,
}

struct Climb<'a> {
    world: &'a mut PhysicsWorld,
    body: BodyId,
    profile: &'a ResolvedProfile,
    casts: &'a mut u32,
}

impl Climb<'_> {
    fn sweep(&mut self, motion: Vec3) -> Result<(Vec3, Option<(HitTarget, Vec3)>), WorldError> {
        *self.casts += 1;
        let options = MoveOptions { skin: self.profile.authored().skin, max_sweeps: 1 };
        let outcome = self.world.move_and_collide(self.body, motion, &options)?;
        Ok((outcome.position, outcome.hits().first().map(|hit| (hit.target, hit.normal))))
    }

    fn face(&mut self, from: Vec3, direction: Vec3) -> Option<RayHit> {
        *self.casts += 1;
        let reach = self.profile.authored().radius + self.profile.authored().climb_reach;
        face(self.world, self.profile, from, direction, reach)
    }

    /// The face the hands hold from `center` into `normal`'s face, within 45° of it.
    fn hold(&mut self, center: Vec3, normal: Vec3) -> Option<RayHit> {
        let held = self.face(hands(self.profile, center), into_face(normal))?;
        (held.normal.dot(normal) >= APPROACH_COS).then_some(held)
    }

    /// Sweeps the capsule to the gap off a face met `distance` along its level inward direction.
    fn settle(&mut self, center: Vec3, normal: Vec3, distance: f32) -> Result<Vec3, WorldError> {
        let gap = distance - (self.profile.authored().radius + hold_gap(self.profile));
        if gap.abs() <= SETTLE {
            return Ok(center);
        }
        Ok(self.sweep(into_face(normal) * gap)?.0)
    }

    /// Round an outside corner: the face round it, seen from just inside where the old one was, back along the
    /// way the climber went. Where it ends up, holding it, if it can.
    fn round(&mut self, center: Vec3, normal: Vec3, way: Vec3) -> Result<Option<(Vec3, Vec3)>, WorldError> {
        let want = self.profile.authored().radius + hold_gap(self.profile);
        let inside = hands(self.profile, center) + into_face(normal) * (want + CORNER_IN);
        let Some(round) = self.face(inside, -way).filter(|round| round.normal.dot(way) >= APPROACH_COS)
        else {
            return Ok(None);
        };
        let level = horizontal(round.normal).unwrap_or(way);
        let there = Vec3::new(round.point.x, center.y, round.point.z) + level * want;
        let delta = there - center;
        let along = way * delta.dot(way);
        let (moved, _) = self.sweep(along)?;
        let (moved, _) = self.sweep(there - moved)?;
        let Some(held) = self.hold(moved, round.normal) else { return Ok(None) };
        Ok(Some((self.settle(moved, held.normal, held.distance)?, held.normal)))
    }

    /// Moves by `motion` from `start` on `normal`'s face, `sideways` the way along it if it goes along it: the
    /// steps 1 to 4 of the [module docs](self).
    fn go(
        &mut self,
        start: Vec3,
        normal: Vec3,
        motion: Vec3,
        sideways: Option<Vec3>,
    ) -> Result<Went, WorldError> {
        let moving = motion.length_squared() >= MIN_MOTION * MIN_MOTION;
        let (center, hit) = if moving { self.sweep(motion)? } else { (start, None) };
        if let Some((target, hit)) = hit {
            if matches!(target, HitTarget::Static(_)) && motion.y < 0.0 && self.profile.walkable(hit) {
                // Climbed down onto the ground: the feet take it from here.
                return Ok(Went::Ground);
            }
            if let Some(way) = sideways.filter(|way| hit.dot(*way) < -APPROACH_COS) {
                // An inside corner: a face across the way it went, which it turns onto.
                let corner = self
                    .face(hands(self.profile, center), way)
                    .filter(|face| -face.normal.dot(way) >= APPROACH_COS);
                if let Some(corner) = corner {
                    let at = self.settle(center, corner.normal, corner.distance)?;
                    return Ok(Went::Held { at, normal: corner.normal, cornered: true });
                }
            }
        }
        if let Some(held) = self.hold(center, normal) {
            let at = self.settle(center, held.normal, held.distance)?;
            return Ok(Went::Held { at, normal: held.normal, cornered: false });
        }
        if let Some(way) = sideways
            && let Some((at, normal)) = self.round(center, normal, way)?
        {
            return Ok(Went::Held { at, normal, cornered: true });
        }
        Ok(Went::Missed)
    }
}

/// One tick of a climber holding `hold`: see the [module docs](self). `false` if it let go, having moved
/// nothing but what took it to the ground.
#[allow(clippy::too_many_arguments, reason = "the controller's step, with the face it holds")]
pub(super) fn step(
    world: &mut PhysicsWorld,
    body: BodyId,
    transform: &mut ActorTransform,
    state: &mut ControllerState,
    mut hold: ClimbHold,
    profile: &ResolvedProfile,
    stick: Vec2,
    delta: SimDuration,
    casts: &mut u32,
) -> Result<bool, WorldError> {
    let pose = world.body_pose(body).ok_or(WorldError::UnknownBody(body))?;
    if let Some(stamina) = profile.stamina() {
        state.stamina_spent += delta;
        if state.stamina_spent >= stamina {
            return Ok(false);
        }
    }
    let start = pose.position;
    let dt = delta.as_secs_f32();
    let reading = read(stick, &hold);
    let (up, side) = axes(hold.normal);
    let mut way = reading.way;
    if hold.lip != Lip::Below && way.y > 0.0 {
        // At the top the hands go no higher: what's left is a top-out's, which is a move.
        way -= up * way.dot(up);
    }
    let motion = way * (profile.authored().climb_speed * dt);
    let moving = motion.length_squared() >= MIN_MOTION * MIN_MOTION;
    // The way it goes along the face, if it goes along it: what a corner turns from.
    let along = motion.dot(side);
    let sideways = (moving && along.abs() > MIN_MOTION).then(|| side * along.signum());
    let rising = motion.dot(up);
    let mut climb = Climb { world, body, profile, casts };
    let mut went = climb.go(start, hold.normal, motion, sideways)?;
    if matches!(went, Went::Missed) && sideways.is_some() && rising > MIN_MOTION {
        // Up and sideways past a side end: the upward part alone may still hold, and only if that misses too
        // is it the top.
        climb.world.set_body_pose(body, Pose::new(start, pose.rotation))?;
        went = climb.go(start, hold.normal, up * rising, None)?;
    }
    let mut center = start;
    match went {
        Went::Ground => return Ok(false),
        Went::Held { at, normal, cornered } => {
            center = at;
            hold.normal = normal;
            if motion.y < 0.0 {
                hold.lip = Lip::Below;
            } else if sideways.is_some() && hold.lip == Lip::Refused {
                // Along the top to somewhere else on it, where a top-out may find room.
                hold.lip = Lip::Reached;
            }
            if cornered {
                // The stick keeps the meaning it had on the face it left until it's released or swings.
                hold.frame = reading.frame;
                hold.latched =
                    if reading.latched == Vec2::ZERO { reading.direction } else { reading.latched };
            } else if reading.latched == Vec2::ZERO {
                hold.frame = hold.normal;
                hold.latched = Vec2::ZERO;
            }
        }
        // Hanging still with nothing to hold, which only a world changing under it does: it falls.
        Went::Missed if !moving => return Ok(false),
        Went::Missed => {
            // Past the tagged face's end, or its top: it stays where it held.
            climb.world.set_body_pose(body, Pose::new(start, pose.rotation))?;
            if rising > MIN_MOTION {
                hold.lip = Lip::Reached;
            }
            hold.frame = reading.frame;
            hold.latched = reading.latched;
        }
    }

    let yaw = det::yaw_of_direction(into_face(hold.normal));
    transform.yaw = yaw;
    let rotation = det::quat_from_rotation_y(yaw);
    if rotation != pose.rotation || center != start {
        climb.world.set_body_pose(body, Pose::new(center, rotation))?;
    }
    state.climb = Some(hold);
    state.traversal = TraversalMode::Climbing;
    state.mode = GroundMode::Airborne;
    state.velocity = (center - start) / dt;
    transform.position = center - profile.feet_to_center();
    Ok(true)
}

/// A vector's horizontal part as a unit vector, if it has one.
fn horizontal(v: Vec3) -> Option<Vec3> {
    let flat = Vec3::new(v.x, 0.0, v.z);
    if flat.length_squared() < 1e-8 { None } else { flat.try_normalize() }
}
