//! What one hit volume does in one step, as plain data: where it is at any sub-tick, and the
//! sub-steps a sweep divides into.

use sapphire_core::Name;
use sapphire_math::{Quat, Vec3, det};
use sapphire_moves::{BakedPath, PathSample};
use sapphire_physics::{GroupMask, Pose, Shape};
use sapphire_time::{SUBTICKS_PER_TICK, SimDuration};

use crate::ActorTransform;
use crate::actors::ActorId;

/// How far a sub-step's straight cast may sag from the arc a root's turn swings a volume along, in
/// metres. A centimetre is well under any hurtbox.
pub const MAX_SAG: f32 = 0.01;

/// The least a turning volume's farthest point may move in a sub-step, in metres, whatever the
/// volume's thickness: it keeps a thin box from costing a cast per degree.
pub const MIN_TURN_STEP: f32 = 0.05;

/// The most casts one volume makes in one step. A swing that wants more makes its last cast straight
/// to the end of its span and counts it ([`crate::HitStats::sweeps_capped`]): it can't tunnel, since
/// a cast is continuous, but that last stretch cuts its corners.
pub const MAX_SUB_STEPS: u64 = 64;

const SUBTICKS: u64 = SUBTICKS_PER_TICK as u64;

/// A hit volume's world pose: `local`, a [`BakedPath`]'s reading in the root's space, through the
/// actor's transform. T1's view draws volumes and hurtboxes with this.
#[must_use]
pub fn volume_pose(root: ActorTransform, local: PathSample) -> Pose {
    Pose::new(
        root.position + det::rotate_y(local.offset(), root.yaw),
        det::quat_normalize(det::quat_from_rotation_y(root.yaw) * local.orientation()),
    )
}

/// What the owner's clock and root did this step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OwnerStep {
    /// How far the owner's clock moved this step (`LocalClock::last_delta`). Zero means it stood
    /// still, for hitstop or a stopped world, whatever the volume's span says: a frozen owner publishes
    /// a volume as one instant, just as one that has only just appeared does.
    pub delta: SimDuration,
    /// Where the step's end falls in the move's time: what the clock reads of this move now. A move
    /// that something cut off this step has no such reading, so give it [`VolumeSweep::to`]: its root
    /// then blends as though the cut came at the step's end.
    pub reading: SimDuration,
    /// Where the root was when the step began, before this tick's movement (`MoveOutput::previous`).
    pub root_before: ActorTransform,
    /// Where the root is now, after this tick's movement (the actor's [`ActorTransform`]).
    pub root_after: ActorTransform,
}

impl OwnerStep {
    /// Whether the owner's clock stood still this step.
    #[must_use]
    pub fn is_frozen(&self) -> bool {
        self.delta == SimDuration::ZERO
    }

    /// The root at a time into the move, by a blend across the part of the move the clock covered this
    /// step: exact at both ends, and held outside them, so a window opening part of the way through
    /// a big step takes its share of the root's motion. Yaw blends as unwrapped radians, so a turn past
    /// a half turn stays one.
    ///
    /// A step of one tick is exact, since tables are linear between ticks. A step covering several is a
    /// straight line across what they did, so a root that starts or stops inside one is off by how far
    /// its table leaves that line. 5× across the end of a root's 7.5 m/s motion falls 7.5 cm short.
    #[must_use]
    pub fn root_at(&self, time: SimDuration) -> ActorTransform {
        let (reading, time) = (self.reading.as_subticks(), time.as_subticks());
        if time >= reading {
            return self.root_after;
        }
        let (behind, delta) = (reading - time, self.delta.as_subticks());
        if behind >= delta {
            return self.root_before;
        }
        let share = 1.0 - behind as f32 / delta as f32;
        let (from, to) = (self.root_before, self.root_after);
        ActorTransform {
            position: from.position + (to.position - from.position) * share,
            yaw: from.yaw + (to.yaw - from.yaw) * share,
        }
    }

    fn is_finite(&self) -> bool {
        let finite = |root: ActorTransform| root.position.is_finite() && root.yaw.is_finite();
        finite(self.root_before) && finite(self.root_after)
    }
}

/// One hit volume this step: everything [`crate::HitQuery::sweep`] reads about it. Plain data, so M2
/// G2's `ActiveVolume` and its actor's `MoveOutput` map straight onto it, as a test's numbers can.
#[derive(Clone, Copy, Debug)]
pub struct VolumeSweep<'a> {
    pub attacker: ActorId,
    /// Which playing of a move this is, of all the attacker's, counting up: `ActiveVolume::serial`.
    /// Hit records are per instance, and a later instance retires the earlier ones'.
    pub instance: u32,
    /// The volume's place in what the attacker published this step: what orders two volumes' contacts,
    /// and what takes [`crate::HitEvent::volume`] back to the `ActiveVolume` it came from.
    pub volume: u16,
    /// The hit group it names: `ActiveVolume::name`. A target is hit once per group, per instance,
    /// per volley.
    pub group: Name,
    /// Whole ticks into the move at which the group re-arms, ascending: a target already hit can be
    /// hit again from each of these on. Empty means once per instance, the usual swing. A multi-hit
    /// spin lists its beats, and they count on the owner's clock, so it lands as many hits at any
    /// time scale.
    pub rearm: &'a [u32],
    /// Its shape. A capsule stands along the path's local +Y.
    pub shape: Shape,
    /// The path it rides, from the move's tables: `ActiveVolume::path`, by name. A sweep reads it at
    /// every sub-step, since the ends alone would cut an arc's corners.
    pub path: &'a BakedPath,
    /// What stops this volume: a sweep that meets a collider in these groups ends there, and stays
    /// ended until the group next re-arms. [`GroupMask::NONE`], the default to author, stops at nothing,
    /// so a swing can't catch on the scenery around a fight; `Group::World` is a sword that stops at a
    /// wall. It counts wherever the volume goes, and the floor is `Group::World` too, so a volume
    /// that says so has to stay clear of it.
    pub blocked_by: GroupMask,
    pub owner: OwnerStep,
    /// The first sub-tick of the move this step sweeps (`ActiveVolume::from`): never before the volume's
    /// window opens, and no earlier than where the last step's sweep stopped.
    pub from: SimDuration,
    /// The last sub-tick it sweeps (`ActiveVolume::to`): never past where the window's arc ends. Equal
    /// to [`Self::from`], the volume is tested where it stands at that one instant.
    pub to: SimDuration,
}

/// One cast of a sweep, as [`VolumeSweep::walk`] divides it. The cast holds one rotation, halfway
/// between `start`'s and `end`'s.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SubStep {
    /// Time into the move at the start.
    pub from: SimDuration,
    /// Time into the move at the end.
    pub to: SimDuration,
    pub start: Pose,
    pub end: Pose,
    /// How far the volume moved in it, in metres: the cast's length, plus how far turning moved its
    /// farthest point. What "nearest" adds up in [`crate::HitEvent::travel`].
    pub motion: f32,
}

impl VolumeSweep<'_> {
    /// The volume's world pose at a time into the move, within this step's span.
    #[must_use]
    pub fn pose_at(&self, time: SimDuration) -> Pose {
        volume_pose(self.owner.root_at(time), self.path.at(time))
    }

    /// How many of [`Self::rearm`]'s ticks a time into the move has reached.
    #[must_use]
    pub fn volley_at(&self, time: SimDuration) -> u32 {
        let reached = self.rearm.partition_point(|&tick| SimDuration::from_ticks(u64::from(tick)) <= time);
        u32::try_from(reached).unwrap_or(u32::MAX)
    }

    pub(super) fn is_valid(&self) -> bool {
        self.shape.is_valid() && self.from <= self.to && self.owner.is_finite()
    }

    /// Calls `visit` with each sub-step of this step's sweep, in order, and returns whether
    /// [`MAX_SUB_STEPS`] cut it short. A frozen owner has none. A span of one instant has one, standing
    /// still.
    ///
    /// **The rule:** every table sample inside the span starts a new stretch, since the table's reading
    /// has a corner there. Between two samples a path's translation is a straight line, so a volume
    /// that doesn't turn needs one cast, however far it goes. Each stretch then divides evenly into
    /// the fewest sub-steps where:
    ///
    /// - **The root's turn:** a straight cast sags no more than [`MAX_SAG`] from the arc that turn
    ///   swings the volume along. From `R (1 - cos(a / 2)) <= R a² / 8`, that's
    ///   `yaw turned * sqrt(R / (8 * MAX_SAG))` of them, `R` being the path's distance from the
    ///   root's vertical axis.
    /// - **Turning:** the volume's farthest point moves no more than the volume is thick
    ///   ([`MIN_TURN_STEP`] at least), so one cast's volume overlaps the next's and nothing thin can
    ///   lie between them. A sphere has no such point. A capsule's is its axis's end, and a box's
    ///   its corner.
    pub fn walk(&self, mut visit: impl FnMut(&SubStep)) -> bool {
        let (from, to) = (self.from.as_subticks(), self.to.as_subticks());
        if self.owner.is_frozen() || from > to {
            return false;
        }
        let mut pose = self.pose_at(self.from);
        if from == to {
            visit(&SubStep { from: self.from, to: self.to, start: pose, end: pose, motion: 0.0 });
            return false;
        }

        let (reach, thickness) = extent(&self.shape);
        let mut left = MAX_SUB_STEPS;
        let mut capped = false;
        let mut at = from;
        while at < to {
            let stretches = (to - 1) / SUBTICKS - at / SUBTICKS + 1;
            // With one cast left, it goes all the way.
            let last = left <= 1 && stretches > 1;
            capped |= last;
            let end = if last { to } else { ((at / SUBTICKS + 1) * SUBTICKS).min(to) };
            let (wanted, turn) = self.wanted_sub_steps(at, end, reach, thickness);
            let room = left.saturating_sub(stretches - 1).max(1);
            capped |= wanted > room;
            let count = wanted.min(room).min(end - at);

            for index in 1..=count {
                let time = if index == count { end } else { at + (end - at) * index / count };
                let time = SimDuration::from_subticks(time);
                let next = self.pose_at(time);
                let motion = (next.position - pose.position).length() + reach * turn / count as f32;
                let previous = SimDuration::from_subticks(at + (end - at) * (index - 1) / count);
                visit(&SubStep { from: previous, to: time, start: pose, end: next, motion });
                pose = next;
            }
            left = left.saturating_sub(count);
            at = end;
        }
        capped
    }

    /// How many sub-steps [`Self::walk`]'s rule wants from one time to another, at least one, and how
    /// far the volume turns, in radians.
    fn wanted_sub_steps(&self, from: u64, to: u64, reach: f32, thickness: f32) -> (u64, f32) {
        let (from, to) = (SimDuration::from_subticks(from), SimDuration::from_subticks(to));
        let (a, b) = (self.path.at(from), self.path.at(to));
        let yaw_turned = (self.owner.root_at(to).yaw - self.owner.root_at(from).yaw).abs();
        let radius = horizontal_length(a.offset()).max(horizontal_length(b.offset()));
        let along_arc = yaw_turned * (radius / (8.0 * MAX_SAG)).sqrt();

        let turn = yaw_turned + path_turn(reach, a.orientation(), b.orientation());
        let turning = reach * turn / thickness.max(MIN_TURN_STEP);

        // A float cast saturates, so a huge turn asks for many and the caller's room decides.
        (along_arc.max(turning).ceil().max(1.0) as u64, turn)
    }
}

/// How far a path turned, where it matters: a volume with no far point has no use for the angle.
fn path_turn(reach: f32, from: Quat, to: Quat) -> f32 {
    if reach == 0.0 { 0.0 } else { det::quat_angle_between(from, to) }
}

fn horizontal_length(offset: Vec3) -> f32 {
    (offset.x * offset.x + offset.z * offset.z).sqrt()
}

/// A shape's farthest point from its center that turning moves, and how thick it is, in metres.
fn extent(shape: &Shape) -> (f32, f32) {
    match *shape {
        Shape::Sphere { radius } => (0.0, radius),
        Shape::Capsule { half_height, radius } => (half_height, radius),
        Shape::Box { half_extents } => (half_extents.length(), half_extents.min_element()),
    }
}
