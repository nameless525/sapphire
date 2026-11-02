//! Hit queries (spec §5.14, §5.22's step 5, M2 D4): which hurtboxes a move's hit volumes touch this
//! tick, swept so nothing tunnels, and deduped per hit group.
//!
//! Owns [`HitQuery`], its [`HitEvent`]s and its records, [`VolumeSweep`] and the sub-step rule
//! ([`VolumeSweep::walk`]), and [`HurtboxId`]. It takes plain inputs and reads no ECS state, so a test
//! can feed it as well as M2 G2's published moves, which [`HitQuery::sweep_published`] and
//! [`published_hurtbox`] turn into those inputs. [`HitQuery`]'s docs say how it behaves.
//!
//! Doesn't own: which volumes are active on a tick, or a move's hurtbox track (M2 G2); damage, poise,
//! reactions, hitstop or cues (the `resolve` module); debug views (M2 T1).

mod book;
mod hurtbox;
mod join;
mod published;
#[cfg(test)]
mod tests;
mod volume;

use std::cmp::Ordering;

use sapphire_core::Name;
use sapphire_ecs::prelude::*;
use sapphire_math::{Vec3, det};
use sapphire_physics::{Group, GroupMask, HitTarget, PhysicsWorld, Pose, QueryFilter, ShapeHit};
use sapphire_time::SimDuration;

use self::book::Book;
pub use self::hurtbox::{HurtboxId, MAX_HURTBOXES, remove_hurtboxes};
pub(crate) use self::join::{place_hurtboxes, sweep_hits};
pub use self::published::{PublishedMoves, hit_shape, published_hurtbox};
pub use self::volume::{MAX_SAG, MAX_SUB_STEPS, MIN_TURN_STEP, OwnerStep, SubStep, VolumeSweep, volume_pose};
use crate::actors::ActorId;
use crate::checksum::StateHasher;

/// The most hits one step queues.
pub const MAX_HIT_EVENTS: usize = 64;

/// The most contacts one step keeps before applying its hit groups. Only each target's nearest in
/// each group and volley takes room, so this bounds targets touched rather than casts made.
pub const MAX_CONTACTS: usize = 128;

/// The most targets all live move instances remember having hit: a whole crowd's worth, for sixteen
/// attackers at once.
pub const MAX_HIT_RECORDS: usize = 256;

/// The most volumes remembered as stopped at once.
pub const MAX_STOPPED_VOLUMES: usize = 32;

/// How many hurtboxes one cast reports. More count in [`HitStats::results_dropped`]: the farthest go.
pub const MAX_CAST_RESULTS: usize = 32;

/// Which way a volume holding still casts, and how far, in metres: a cast needs a direction and some
/// length, and a tenth of a millimetre is no one's idea of moving.
const STILL_DIRECTION: Vec3 = Vec3::Y;
const STILL_CAST: f32 = 1e-4;

/// A cast shorter than this holds still.
const MIN_CAST: f32 = 1e-5;

/// A volume touching a target for the first time in a hit group: what G3 turns into damage, and T1's
/// view draws. All in world space, except [`Self::at`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitEvent {
    pub attacker: ActorId,
    /// Which playing of the attacker's move.
    pub instance: u32,
    pub target: ActorId,
    /// Which of the target's hurtboxes was touched first.
    pub hurtbox: u8,
    pub group: Name,
    /// How many times the group had re-armed: 0 for its first hit.
    pub volley: u32,
    /// The volume's place in what the attacker published this step ([`VolumeSweep::volume`]).
    pub volume: u16,
    /// On the hurtbox's surface.
    pub point: Vec3,
    /// Away from the hurtbox, toward the volume. A box that starts inside a target has no true one:
    /// it's back along the cast, or from the target's center to the volume's if it's holding still.
    pub normal: Vec3,
    /// When it touched: time into the move on the attacker's clock, to the sub-tick.
    pub at: SimDuration,
    /// How far the volume had moved this step when it touched, in metres (see [`SubStep::motion`]).
    /// Zero for a volume that starts the step touching.
    pub travel: f32,
}

/// What hit queries have done so far. Counts only, deterministic, and not in the checksum, since
/// nothing reads them back; `sweeps_capped` and the `_dropped` and `_refused` counts are what to watch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HitStats {
    /// Volumes swept. A frozen one isn't.
    pub sweeps: u64,
    /// Casts those sweeps made.
    pub sub_steps: u64,
    /// Volumes passed over because their owner's clock stood still.
    pub frozen_skips: u64,
    /// Sweeps [`MAX_SUB_STEPS`] cut short.
    pub sweeps_capped: u64,
    /// Volumes refused for an invalid shape, an empty span or a non-finite root.
    pub invalid_volumes: u64,
    /// Hurtboxes casts touched, before records and hit groups.
    pub touches: u64,
    /// Hits queued.
    pub events: u64,
    /// Volumes a collider stopped ([`VolumeSweep::blocked_by`]).
    pub volumes_stopped: u64,
    /// Touches of something in the hurtbox group whose ID isn't a [`HurtboxId`]'s, which hits nobody.
    pub unowned_touches: u64,
    /// Touches a cast had no room for ([`MAX_CAST_RESULTS`]). These hits are lost, unless a later
    /// cast finds them.
    pub results_dropped: u64,
    /// Contacts past [`MAX_CONTACTS`], lost the same way.
    pub contacts_dropped: u64,
    /// Hits past [`MAX_HIT_EVENTS`], which land next tick if the volume still touches.
    pub events_dropped: u64,
    /// Hits refused because [`MAX_HIT_RECORDS`] is full.
    pub hits_refused: u64,
    /// Stopped volumes past [`MAX_STOPPED_VOLUMES`], which sweep on from their next step.
    pub stops_dropped: u64,
}

/// Hit queries: which hurtboxes moves' hit volumes touch each tick, swept so nothing tunnels, and
/// deduped per hit group. Built once with room for everything, so nothing in a step allocates (rule
/// R6), and everything that can change a later result is in the simulation's checksum.
///
/// # A step
///
/// In [`CombatSet::Hits`](crate::CombatSet::Hits), after every actor has moved:
///
/// 1. [`Self::sweep`] for each active volume, by [`ActorId`], then instance, then volume index. Each
///    makes its casts ([`VolumeSweep::walk`] states the rule) and notes what they touched.
///    [`Self::sweep_published`] does this for all an actor's moves published.
/// 2. [`Self::finish_step`] once, even with nothing swept. It puts the contacts in order, applies the
///    hit groups and fills [`Self::events`], which [`CombatSet::Resolve`](crate::CombatSet::Resolve)
///    reads. They stay there until the next step's `finish_step`.
///
/// # Time
///
/// A sweep runs on the owner's clock, in sub-ticks into the move. It covers [`VolumeSweep::from`] to
/// [`VolumeSweep::to`], which whatever publishes the volume clips to the volume's window, so a window
/// opening this tick starts at its first active sub-tick and can't reach back. Each sweep picks up
/// where the last one stopped, however a time scale divides them. A frozen owner
/// ([`OwnerStep::is_frozen`]) makes no casts at all, so it can't hit anything new, even something that
/// walks into it, and can't hit anything again.
///
/// # Hurtboxes
///
/// They're bodies in `Group::Hurtbox`, in the [`PhysicsWorld`], with [`HurtboxId`]s for IDs. A move's
/// hurtbox track sets them with [`HurtboxId::set`], through [`published_hurtbox`], in
/// [`CombatSet::Movement`](crate::CombatSet::Movement), which runs before `Hits`, and the physics world
/// applies a pose at once, so a sweep reads this
/// tick's. A target is tested only where it stands now: a target and a swing crossing each other in
/// one tick meet or miss by the target's end pose. An attacker never touches its own hurtboxes.
///
/// # Order
///
/// Events are nearest first: least [`HitEvent::travel`], how far the volume had moved this step when
/// it touched. Ties go by volume (attacker, then instance, then volume index), then by target, then
/// by hurtbox. Hit groups apply in that order, so when two volumes of a group touch one target, the
/// nearer gets the credit, and when a volume touches two hurtboxes of one target, the nearer does.
///
/// # Hit groups
///
/// A target is hit once per group, per move instance, until the group re-arms. A group re-arms at the
/// ticks its volume lists ([`VolumeSweep::rearm`]), and the number of those a contact has reached is
/// its volley, so a contact counts if its volley is later than the target's last. Records go when a
/// later instance of the same attacker sweeps, or with [`Self::forget_actor`].
///
/// # Overflow
///
/// Every limit counts in [`HitStats`] and carries on. A full event queue writes no record, so the hit
/// lands next tick if the volume still touches it. A full ledger refuses the hit, since a hit with no
/// record would land again on every tick. Both are far above a fight's needs, so non-zero counts mean
/// they should grow.
///
/// # Example
///
/// ```
/// use sapphire_core::Name;
/// use sapphire_gameplay::{ActorId, ActorTransform, HitQuery, HurtboxId, OwnerStep, VolumeSweep};
/// use sapphire_math::Vec3;
/// use sapphire_moves::{BakedPath, PathSample};
/// use sapphire_physics::{GroupMask, PhysicsWorld, Shape};
/// use sapphire_time::SimDuration;
///
/// let (attacker, target) = (ActorId::from_raw(1).unwrap(), ActorId::from_raw(2).unwrap());
/// let root = ActorTransform::default();
///
/// // A target's chest, a metre up and two metres in front of the attacker.
/// let mut world = PhysicsWorld::new(Vec::new(), 8).unwrap();
/// let chest = HurtboxId::new(target, 0).unwrap();
/// let local = PathSample { translation: [0.0, 1.0, 2.0], ..PathSample::IDENTITY };
/// chest.set(&mut world, Some(Shape::Sphere { radius: 0.25 }), root, local).unwrap();
///
/// // A fist that goes three metres forward in a tick, right through it.
/// let samples = [0.0, 3.0].map(|z| PathSample { translation: [0.0, 1.0, z], ..PathSample::IDENTITY });
/// let fist = BakedPath::new(Name::new("fist"), samples.to_vec());
///
/// let mut hits = HitQuery::new();
/// let tick = SimDuration::from_ticks(1);
/// hits.sweep(
///     &world,
///     &VolumeSweep {
///         attacker,
///         instance: 1,
///         volume: 0,
///         group: Name::new("jab"),
///         rearm: &[],
///         shape: Shape::Sphere { radius: 0.05 },
///         path: &fist,
///         blocked_by: GroupMask::NONE,
///         owner: OwnerStep { delta: tick, reading: tick, root_before: root, root_after: root },
///         from: SimDuration::ZERO,
///         to: tick,
///     },
/// );
/// hits.finish_step();
///
/// let event = hits.events()[0];
/// assert_eq!(event.target, target);
/// // The fist's surface reaches the chest's when its center is 1.7 metres along.
/// assert!((event.travel - 1.7).abs() < 1e-3);
/// assert!(event.point.abs_diff_eq(Vec3::new(0.0, 1.0, 1.75), 1e-3));
/// ```
#[derive(Resource, Debug)]
pub struct HitQuery {
    /// Who each live instance has hit: `(group, target)`, with the latest volley.
    records: Book<(Name, ActorId)>,
    /// Which volumes a collider stopped: `(group, path)`, with the volley it stopped in. A volume's
    /// place in a step's list moves as other windows open and close, and these two don't.
    stopped: Book<(Name, Name)>,
    /// This step's contacts, emptied by [`Self::finish_step`].
    contacts: Vec<HitEvent>,
    /// The latest step's hits, in order.
    events: Vec<HitEvent>,
    /// A cast's out buffer. Only ever read back within one cast.
    results: Box<[ShapeHit; MAX_CAST_RESULTS]>,
    stats: HitStats,
}

impl Default for HitQuery {
    fn default() -> Self {
        Self::new()
    }
}

impl HitQuery {
    /// Empty, with room reserved for every limit.
    #[must_use]
    pub fn new() -> Self {
        Self {
            records: Book::with_capacity(MAX_HIT_RECORDS),
            stopped: Book::with_capacity(MAX_STOPPED_VOLUMES),
            contacts: Vec::with_capacity(MAX_CONTACTS),
            events: Vec::with_capacity(MAX_HIT_EVENTS),
            results: Box::new([ShapeHit::NONE; MAX_CAST_RESULTS]),
            stats: HitStats::default(),
        }
    }

    /// Sweeps one volume through this step's span and notes what it touches. See the
    /// [type's docs](Self) for the order to call this in.
    pub fn sweep(&mut self, world: &PhysicsWorld, volume: &VolumeSweep<'_>) {
        profiling::scope!("HitQuery::sweep");
        // Sweeps come in instance order, so an earlier instance's contacts this step were checked
        // against its records as they were made, and nothing still needs what goes here.
        self.records.retire(volume.attacker, volume.instance);
        self.stopped.retire(volume.attacker, volume.instance);

        if volume.owner.is_frozen() {
            self.stats.frozen_skips += 1;
            return;
        }
        if !volume.is_valid() {
            self.stats.invalid_volumes += 1;
            return;
        }
        self.stats.sweeps += 1;

        let filter = match HurtboxId::bodies_of(volume.attacker) {
            Some((first, last)) => QueryFilter::new(Group::Hurtbox.mask()).excluding_bodies(first, last),
            None => QueryFilter::new(Group::Hurtbox.mask()),
        };
        let stop = (volume.group, volume.path.name());
        let mut stopped_in = self.stopped.volley_of(volume.attacker, volume.instance, stop);
        let mut travelled = 0.0;
        let capped = volume.walk(|sub| {
            self.cast(world, volume, &filter, sub, travelled, &mut stopped_in);
            travelled += sub.motion;
        });
        self.stats.sweeps_capped += u64::from(capped);
    }

    /// One sub-step's casts: what stops the volume, then what it touches on the way there.
    fn cast(
        &mut self,
        world: &PhysicsWorld,
        volume: &VolumeSweep<'_>,
        filter: &QueryFilter,
        sub: &SubStep,
        travelled: f32,
        stopped_in: &mut Option<u32>,
    ) {
        // A stopped volume starts again when its group re-arms.
        if stopped_in.is_some_and(|volley| volume.volley_at(sub.from) <= volley) {
            return;
        }
        self.stats.sub_steps += 1;

        let delta = sub.end.position - sub.start.position;
        let length = delta.length();
        let still = length < MIN_CAST;
        let (direction, mut distance) =
            if still { (STILL_DIRECTION, STILL_CAST) } else { (delta / length, length) };
        // Halfway between the two rotations: the rule's limit on turning bounds the error each way.
        let rotation = det::quat_nlerp(sub.start.rotation, sub.end.rotation, 0.5);
        let pose = Pose::new(sub.start.position, rotation);

        if volume.blocked_by != GroupMask::NONE {
            let walls = QueryFilter { groups: volume.blocked_by, ..*filter };
            if let Some(wall) = world.cast_shape(&volume.shape, pose, direction, distance, &walls) {
                distance = wall.distance;
                let volley = volume.volley_at(time_along(sub, if still { 0.0 } else { distance / length }));
                *stopped_in = Some(volley);
                self.stats.volumes_stopped += 1;
                let stop = (volume.group, volume.path.name());
                if !self.stopped.write(volume.attacker, volume.instance, stop, volley) {
                    self.stats.stops_dropped += 1;
                }
            }
        }

        let found =
            world.cast_shape_all(&volume.shape, pose, direction, distance, filter, &mut *self.results);
        self.stats.results_dropped += (found.found - found.written) as u64;
        self.stats.touches += found.written as u64;
        for index in 0..found.written {
            let mut hit = self.results[index];
            let HitTarget::Body(body) = hit.target else {
                self.stats.unowned_touches += 1;
                continue;
            };
            let Some(hurtbox) = HurtboxId::from_body(body) else {
                self.stats.unowned_touches += 1;
                continue;
            };
            if still && hit.started_overlapping && hit.normal == -direction {
                // Physics' answer for a box with no core to measure from, back along a cast that went
                // nowhere: the line between the centers says more.
                let center = world.body_pose(body).map_or(hit.point, |pose| pose.position);
                hit.normal = (sub.start.position - center).try_normalize().unwrap_or(direction);
            }
            let share = if still { 0.0 } else { (hit.distance / length).min(1.0) };
            self.note_contact(volume, sub, hurtbox, &hit, share, travelled);
        }
    }

    /// Keeps a touch if the records let it through, and only its target's nearest per group and volley.
    fn note_contact(
        &mut self,
        volume: &VolumeSweep<'_>,
        sub: &SubStep,
        hurtbox: HurtboxId,
        hit: &ShapeHit,
        share: f32,
        travelled: f32,
    ) {
        let at = time_along(sub, share);
        let volley = volume.volley_at(at);
        let target = hurtbox.actor();
        let key = (volume.group, target);
        if self.records.volley_of(volume.attacker, volume.instance, key).is_some_and(|last| last >= volley) {
            return;
        }
        let contact = HitEvent {
            attacker: volume.attacker,
            instance: volume.instance,
            target,
            hurtbox: hurtbox.index(),
            group: volume.group,
            volley,
            volume: volume.volume,
            point: hit.point,
            normal: hit.normal,
            at,
            travel: travelled + sub.motion * share,
        };
        let same = self.contacts.iter().position(|known| {
            known.attacker == contact.attacker
                && known.instance == contact.instance
                && known.group == contact.group
                && known.volley == contact.volley
                && known.target == contact.target
        });
        match same {
            Some(slot) => {
                if order(&contact, &self.contacts[slot]) == Ordering::Less {
                    self.contacts[slot] = contact;
                }
            }
            None if self.contacts.len() >= MAX_CONTACTS => self.stats.contacts_dropped += 1,
            None => self.contacts.push(contact),
        }
    }

    /// Ends the step's sweeps: orders their contacts, applies the hit groups and replaces
    /// [`Self::events`]. Call it every step.
    pub fn finish_step(&mut self) {
        profiling::scope!("HitQuery::finish_step");
        self.contacts.sort_unstable_by(order);
        self.events.clear();
        for contact in self.contacts.drain(..) {
            let key = (contact.group, contact.target);
            let last = self.records.volley_of(contact.attacker, contact.instance, key);
            if last.is_some_and(|last| last >= contact.volley) {
                continue;
            }
            if self.events.len() >= MAX_HIT_EVENTS {
                self.stats.events_dropped += 1;
                continue;
            }
            if self.records.write(contact.attacker, contact.instance, key, contact.volley) {
                self.events.push(contact);
                self.stats.events += 1;
            } else {
                self.stats.hits_refused += 1;
            }
        }
    }

    /// The latest step's hits, in the order the [type's docs](Self) state.
    #[must_use]
    pub fn events(&self) -> &[HitEvent] {
        &self.events
    }

    /// Forgets everything about an actor, as attacker and as target, when it despawns. IDs are never
    /// reused, so this only gives the room back. Allocates nothing.
    pub fn forget_actor(&mut self, actor: ActorId) {
        self.records.forget(|entry| entry.attacker == actor || entry.key.1 == actor);
        self.stopped.forget(|entry| entry.attacker == actor);
    }

    /// How many targets are on record now.
    #[must_use]
    pub fn record_count(&self) -> usize {
        self.records.entries.len()
    }

    /// The counters so far (rule R9).
    #[must_use]
    pub fn stats(&self) -> HitStats {
        self.stats
    }

    /// Writes everything that can change a later result, as canonical bytes. The books are sorted and
    /// the events in their stated order, so none of this depends on the order things happened in
    /// memory.
    pub(crate) fn write_canonical(&self, hasher: &mut StateHasher) {
        self.records.write_canonical(hasher, |hasher, (group, target)| {
            hasher.u64(group.as_u64());
            hasher.u32(target.raw());
        });
        self.stopped.write_canonical(hasher, |hasher, (group, path)| {
            hasher.u64(group.as_u64());
            hasher.u64(path.as_u64());
        });
        hasher.u32(self.contacts.len() as u32);
        for event in self.contacts.iter().chain(&self.events) {
            write_event(hasher, event);
        }
        hasher.u32(self.events.len() as u32);
    }
}

fn write_event(hasher: &mut StateHasher, event: &HitEvent) {
    hasher.u32(event.attacker.raw());
    hasher.u32(event.instance);
    hasher.u32(event.target.raw());
    hasher.u8(event.hurtbox);
    hasher.u64(event.group.as_u64());
    hasher.u32(event.volley);
    hasher.u16(event.volume);
    hasher.vec3(event.point);
    hasher.vec3(event.normal);
    hasher.u64(event.at.as_subticks());
    hasher.f32(event.travel);
}

/// The time a share of the way through a sub-step, to the sub-tick.
fn time_along(sub: &SubStep, share: f32) -> SimDuration {
    let (from, to) = (sub.from.as_subticks(), sub.to.as_subticks());
    let span = to - from;
    SimDuration::from_subticks(from + ((span as f32 * share).round() as u64).min(span))
}

/// The stated order, total so sorting needs no stability: nearest, then the volume (attacker, instance,
/// index), then target, then hurtbox. What follows only separates contacts that can't both matter.
fn order(a: &HitEvent, b: &HitEvent) -> Ordering {
    a.travel
        .total_cmp(&b.travel)
        .then_with(|| a.attacker.cmp(&b.attacker))
        .then_with(|| a.instance.cmp(&b.instance))
        .then_with(|| a.volume.cmp(&b.volume))
        .then_with(|| a.target.cmp(&b.target))
        .then_with(|| a.hurtbox.cmp(&b.hurtbox))
        .then_with(|| a.at.cmp(&b.at))
        .then_with(|| a.group.cmp(&b.group))
        .then_with(|| a.volley.cmp(&b.volley))
}
