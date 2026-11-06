//! Move-and-collide: the kinematic sweep character controllers build on.
//!
//! A move pushes the body out of anything it overlaps, then sweeps along the motion, stopping a
//! [skin](MoveOptions::skin) short of what it hits and sliding along it. Everything a controller
//! decides from comes back in the [`MoveOutcome`]: each surface's normal and how far along the
//! motion it was met. What the controller decides (what counts as ground, what a step is, when to
//! stop sliding) is M2 G1's, built from these and the queries.

use glam::Vec3;

use crate::narrow::GRAZE;
use crate::query::{Penetration, Selector, ShapeHit, shape_hit};
use crate::shape::{Pose, Shape};
use crate::stats::Counters;
use crate::world::{BodyId, HitTarget, PhysicsWorld, WorldError};

/// The most surfaces one move reports.
pub const MAX_MOVE_HITS: usize = 8;

/// Motion shorter than this isn't worth a sweep, in metres.
const MIN_MOTION: f32 = 1e-5;
/// How many times a move pushes out of overlaps before going on regardless.
const MAX_OVERLAP_PUSHES: u32 = 4;
/// An overlap shallower than this doesn't need resolving, in metres: rounding leaves this much.
const MIN_OVERLAP: f32 = 1e-5;

/// How a move behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveOptions {
    /// The gap kept from surfaces, in metres. A body left exactly on a surface can't tell moving
    /// along it from moving into it, so it stays this far off.
    pub skin: f32,
    /// How many sweeps a move may make, each after sliding along the last surface. 1 stops at the
    /// first hit, leaving the rest in [`MoveOutcome::remaining`]; at most [`MAX_MOVE_HITS`].
    pub max_sweeps: u32,
}

impl Default for MoveOptions {
    fn default() -> Self {
        Self { skin: 0.01, max_sweeps: 4 }
    }
}

/// A surface a move hit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveHit {
    pub target: HitTarget,
    /// Away from the surface, toward the body: `normal.y` near 1 is ground.
    pub normal: Vec3,
    /// On the surface.
    pub point: Vec3,
    /// How far along the whole motion this hit came, in 0..=1: the path travelled before it, over
    /// the motion's length.
    pub fraction: f32,
    /// The body started this sweep overlapping the surface.
    pub started_overlapping: bool,
}

impl MoveHit {
    const NONE: Self = Self {
        target: HitTarget::Body(BodyId(u32::MAX)),
        normal: Vec3::Y,
        point: Vec3::ZERO,
        fraction: 0.0,
        started_overlapping: false,
    };
}

/// What a move did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MoveOutcome {
    /// Where the body's origin ended up.
    pub position: Vec3,
    /// The whole displacement, overlap pushes included.
    pub moved: Vec3,
    /// Motion not travelled when the sweeps ran out: zero after a clear run. Not yet slid along
    /// the last surface, so with `max_sweeps: 1` this is the part a wall stopped.
    pub remaining: Vec3,
    /// The part of [`Self::moved`] that pushed the body out of overlaps it started in. Pushes back and
    /// forth can add up to nothing, so [`Self::started_overlapping`] is what says whether there were any.
    pub overlap_push: Vec3,
    /// The pushes ran out with the body still in something: a gap narrower than it, say, where each one
    /// out of one side goes into the other. The move goes on from where they left it.
    pub still_overlapping: bool,
    started_overlapping: bool,
    hits: [MoveHit; MAX_MOVE_HITS],
    hit_count: u8,
}

impl MoveOutcome {
    /// The surfaces hit, in the order met.
    pub fn hits(&self) -> &[MoveHit] {
        &self.hits[..usize::from(self.hit_count)]
    }

    /// Whether the body started overlapping something.
    pub fn started_overlapping(&self) -> bool {
        self.started_overlapping
    }
}

impl PhysicsWorld {
    /// Moves a sphere or capsule by `motion`, stopping for what its groups meet and sliding along
    /// it, and returns what happened. The sweeps are continuous, so no speed tunnels through
    /// anything, however thin. Rotation is the caller's, through [`Self::set_body_pose`].
    ///
    /// Motion, skin, derived sweep lengths and resulting bounds must be finite and representable.
    /// An invalid move returns [`WorldError::InvalidBody`] without changing the stored pose.
    ///
    /// Boxes can't move this way: resolving an overlap measures from a shape's core, which a box
    /// has none of ([`WorldError::CannotMove`]).
    pub fn move_and_collide(
        &mut self,
        id: BodyId,
        motion: Vec3,
        options: &MoveOptions,
    ) -> Result<MoveOutcome, WorldError> {
        profiling::scope!("PhysicsWorld::move_and_collide");
        let slot = self.body_slot(id).map_err(|_| WorldError::UnknownBody(id))?;
        let body = self.bodies[slot];
        if matches!(body.shape, Shape::Box { .. }) {
            return Err(WorldError::CannotMove(id));
        }
        let total = motion.length();
        let skin = options.skin.max(0.0);
        if !motion.is_finite()
            || !total.is_finite()
            || !options.skin.is_finite()
            || !(total + 4.0 * skin).is_finite()
            || !(body.pose.position + motion).is_finite()
        {
            return Err(WorldError::InvalidBody(id));
        }
        Counters::add(&self.counters.moves, 1);

        let selector = Selector::Mover { id, groups: body.groups };
        let start = body.pose.position;
        let mut position = start;
        let mut outcome = MoveOutcome {
            position,
            moved: Vec3::ZERO,
            remaining: Vec3::ZERO,
            overlap_push: Vec3::ZERO,
            still_overlapping: false,
            started_overlapping: false,
            hits: [MoveHit::NONE; MAX_MOVE_HITS],
            hit_count: 0,
        };

        let overlap_at = |position: Vec3| {
            let at = with_position(&body.parry_pose, position);
            self.deepest_overlap(&body.convex, &at, selector).filter(|overlap| overlap.depth >= MIN_OVERLAP)
        };
        let mut pushes = 0;
        while pushes < MAX_OVERLAP_PUSHES
            && let Some(Penetration { normal, depth, .. }) = overlap_at(position)
        {
            position += normal * (depth + skin);
            if !body.accepts_position(position) {
                return Err(WorldError::InvalidBody(id));
            }
            pushes += 1;
        }
        outcome.overlap_push = position - start;
        outcome.started_overlapping = pushes > 0;
        // Only a squeezed body looks again, so a move through open space pays nothing for this.
        outcome.still_overlapping = pushes == MAX_OVERLAP_PUSHES && overlap_at(position).is_some();
        if outcome.still_overlapping {
            Counters::add(&self.counters.overlaps_unresolved, 1);
        } else if outcome.started_overlapping {
            Counters::add(&self.counters.overlaps_resolved, 1);
        }

        let sweeps = options.max_sweeps.clamp(1, MAX_MOVE_HITS as u32);
        let mut remaining = motion;
        let mut travelled = 0.0;
        let mut last_normal: Option<Vec3> = None;
        let mut sweeps_made = 0;
        for sweep_index in 0..sweeps {
            let length = remaining.length();
            if !length.is_finite() || !body.accepts_position(position) {
                return Err(WorldError::InvalidBody(id));
            }
            if length < MIN_MOTION {
                remaining = Vec3::ZERO;
                break;
            }
            sweeps_made += 1;
            let direction = remaining / length;
            let at = with_position(&body.parry_pose, position);
            // A little past the motion, so a surface just beyond it still holds the skin.
            let reach = length + 4.0 * skin;
            let Some((target, hit)) = self.first_sweep(&body.convex, &at, direction, reach, selector, true)
            else {
                position += remaining;
                remaining = Vec3::ZERO;
                break;
            };

            // The skin is a gap along the normal, so at an angle it takes more of the path.
            let approach = (-direction.dot(hit.normal)).max(GRAZE);
            let advance = (hit.distance - skin / approach).clamp(0.0, length);
            if advance >= length {
                // The surface lies past the motion: only the skin's reach found it.
                position += remaining;
                remaining = Vec3::ZERO;
                break;
            }
            position += direction * advance;
            remaining -= direction * advance;
            travelled += advance;
            let count = usize::from(outcome.hit_count);
            outcome.hits[count] = MoveHit {
                target,
                normal: hit.normal,
                point: hit.point,
                fraction: if total > 0.0 { (travelled / total).min(1.0) } else { 0.0 },
                started_overlapping: hit.overlapping,
            };
            outcome.hit_count += 1;

            if sweep_index + 1 < sweeps {
                remaining = slide(remaining, hit.normal, last_normal);
                last_normal = Some(hit.normal);
            }
        }

        Counters::add(&self.counters.move_sweeps, sweeps_made);
        Counters::add(&self.counters.move_hits, u32::from(outcome.hit_count));
        outcome.position = position;
        outcome.moved = position - start;
        outcome.remaining = remaining;
        if !position.is_finite()
            || !outcome.moved.is_finite()
            || !remaining.is_finite()
            || !outcome.overlap_push.is_finite()
        {
            return Err(WorldError::InvalidBody(id));
        }
        self.bodies[slot].set_pose(Pose::new(position, body.pose.rotation))?;
        Ok(outcome)
    }

    /// What a body would hit moving `direction` from where it stands, as [`Self::move_and_collide`]
    /// sees it: its own shape and groups, itself excepted, and what it's moving away from ignored.
    /// For ground probes and step-up checks; `offset` starts it from elsewhere, such as over a step.
    pub fn cast_body(
        &self,
        id: BodyId,
        offset: Vec3,
        direction: Vec3,
        max_distance: f32,
    ) -> Result<Option<ShapeHit>, WorldError> {
        let slot = self.body_slot(id).map_err(|_| WorldError::UnknownBody(id))?;
        let body = &self.bodies[slot];
        let Some(direction) = direction.try_normalize() else {
            return Ok(None);
        };
        let selector = Selector::Mover { id, groups: body.groups };
        let start = with_position(&body.parry_pose, body.pose.position + offset);
        Counters::add(&self.counters.shape_casts, 1);
        let hit = self.first_sweep(&body.convex, &start, direction, max_distance, selector, true);
        Ok(hit.map(|(target, sweep)| shape_hit(target, sweep, max_distance)))
    }
}

fn with_position(pose: &parry3d::math::Pose, position: Vec3) -> parry3d::math::Pose {
    parry3d::math::Pose::from_parts(position, pose.rotation)
}

/// What's left of `remaining` after sliding along a surface. In the crease with the last surface,
/// if sliding along this one would run back into it, which keeps a body from jittering between
/// two walls.
fn slide(remaining: Vec3, normal: Vec3, last_normal: Option<Vec3>) -> Vec3 {
    let along = remaining - normal * remaining.dot(normal);
    match last_normal {
        Some(last) if along.dot(last) < 0.0 => match last.cross(normal).try_normalize() {
            Some(crease) => crease * along.dot(crease),
            None => Vec3::ZERO,
        },
        _ => along,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliding_removes_only_the_part_into_the_surface() {
        let slid = slide(Vec3::new(3.0, -4.0, 0.0), Vec3::Y, None);
        assert_eq!(slid, Vec3::new(3.0, 0.0, 0.0));
    }

    #[test]
    fn a_second_wall_turns_a_slide_into_its_crease() {
        // Into a corner between +x and +z walls: along the first wall, then the second would send
        // it back through the first, so only the crease (Y here) remains.
        let slid = slide(Vec3::new(-1.0, 2.0, -1.0), Vec3::X, Some(Vec3::Z));
        assert_eq!(slid, Vec3::new(0.0, 2.0, 0.0));
    }

    #[test]
    fn parallel_walls_stop_a_slide_that_runs_back() {
        let slid = slide(Vec3::new(1.0, 0.0, 0.0), Vec3::NEG_X, Some(Vec3::new(1.0, 0.0, 0.0)));
        assert_eq!(slid, Vec3::ZERO);
    }
}
