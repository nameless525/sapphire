//! Keeping brains apart (M3 P3), so a crowd after one foe spreads round it rather than stacking on it or
//! queueing behind whoever's in front.
//!
//! A brain's stick says where it wants to go, and this leans it, in [`crate::brain::think`], after the
//! brain has decided. Three things lean it, from each other brain nearer than its
//! [`apart`](sapphire_moves::BrainDef::apart):
//!
//! - **A push away,** stronger the nearer they are, and nothing at `apart`. Against a want to go on, that
//!   comes to rest at a gap, so a crowd stands apart rather than leaning on its capsules.
//! - **A step aside** from someone in the way: to whichever side of it it's on already, most when it's
//!   dead ahead, and nothing once it's clear to the side. A push alone holds a file in line, since all it
//!   can do to someone straight behind is slow them, and a step aside is what fans one out. It's balanced
//!   when there's someone either side, so a full ring comes to rest rather than turning for ever.
//! - **A hold,** taking its want out of the stick while someone's in the way. Without it the back of a
//!   crowd leans through its pushes onto the front, and those at the front end up touching.
//!
//! **Why not ORCA:** this is what the scout's notes asked of a first version. Nothing here looks ahead, so
//! two walking at each other meet before they part. Capsules still block (`controller`), so nobody goes
//! through anyone.
//!
//! # What it reads
//!
//! [`Crowd`] is who stood where at the end of the last step, taken once at the start of `think`, so which
//! brain goes first changes nothing here and nothing leans on a move made this tick. It's scratch,
//! rewritten every tick, so it isn't in the checksum. Its order is ID order. The arithmetic is scalars,
//! `+ - * /` and `sqrt`. The only thing an ID decides is which side two exactly in line step to, and which
//! way two exactly on top of each other part: each by the pair, so the order of the list has no part in it.
//!
//! # What it costs
//!
//! Every brain against every other, over a flat list, and a pair too far apart along X costs one compare:
//! see spec §5.19 for the figures. At 64 that's nothing to speak of. A zone with hundreds thinking at once
//! would want them in cells, which nothing here prevents.

use sapphire_ecs::prelude::*;
use sapphire_math::Vec3;

use crate::actions::Stick;
use crate::actors::ActorId;

/// How hard the crowd leans on a stick, as tilt for a neighbour at closeness 1, closeness being 0 at `apart`
/// and 1 on top of them. A want of 0.7 alone against one neighbour dead ahead rests at closeness 0.23, which
/// for an `apart` of 1.2 m is 0.92 m: a gap of 32 cm between the arena's capsules, 0.3 m in radius.
const LEAN: f32 = 3.0;

/// How much of its want someone in the way takes away, for a neighbour dead ahead at closeness 1: all of
/// it from closeness 0.25, which for an `apart` of 1.2 m is 0.9 m.
const HOLD: f32 = 4.0;

/// Under this much tilt with nothing else to do, it isn't worth walking: what stops a crowd creeping.
const STAND: f32 = 0.06;

/// Under this many metres across the ground, two are on top of each other, with no direction between them.
const ON_TOP: f32 = 1e-3;

/// More than this many metres apart up or down, they're on different floors.
const STOREY: f32 = 1.5;

/// Who's standing where, this tick. A resource, sized for the pool once, so it never grows. See the
/// [module docs](self).
#[derive(Resource)]
pub(crate) struct Crowd {
    /// In ID order.
    standing: Vec<(ActorId, Vec3)>,
}

impl Crowd {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        Self { standing: Vec::with_capacity(capacity) }
    }

    /// Forgets last tick's.
    pub(crate) fn clear(&mut self) {
        self.standing.clear();
    }

    /// One more, of no more than the pool holds, in ID order.
    pub(crate) fn add(&mut self, id: ActorId, position: Vec3) {
        debug_assert!(self.standing.len() < self.standing.capacity(), "it's sized for the pool");
        self.standing.push((id, position));
    }

    /// `stick` for `me`, leaned by everyone but itself and `foe`, its target, which its spacing already
    /// keeps its distance from. It goes no faster for it than `most`, or than it was going. A stick with
    /// nobody near comes back as it went in, to the bit.
    pub(crate) fn lean(
        &self,
        me: ActorId,
        foe: Option<ActorId>,
        at: Vec3,
        apart: f32,
        stick: Stick,
        most: f32,
    ) -> Stick {
        let want = stick.to_vec2();
        let lean = lean_on(me, foe, at, apart, [want.x, want.y], &self.standing);
        if lean == [0.0, 0.0] {
            return stick;
        }
        let (mut x, mut y) = (want.x + lean[0], want.y + lean[1]);
        let wanted = (want.x * want.x + want.y * want.y).sqrt();
        let length = (x * x + y * y).sqrt();
        if wanted == 0.0 && length < STAND {
            return stick;
        }
        let most = most.max(wanted);
        if length > most {
            let scale = most / length;
            (x, y) = (x * scale, y * scale);
        }
        Stick::from_axes(x, y)
    }
}

/// How much `me`, at `at`, is leaned on by `others`, as tilt on the ground's X and Z, given the way it
/// wants to go. See the [module docs](self).
fn lean_on(
    me: ActorId,
    foe: Option<ActorId>,
    at: Vec3,
    apart: f32,
    want: [f32; 2],
    others: &[(ActorId, Vec3)],
) -> [f32; 2] {
    if apart <= 0.0 {
        return [0.0, 0.0];
    }
    let wanted = (want[0] * want[0] + want[1] * want[1]).sqrt();
    let heading = if wanted > 0.0 { [want[0] / wanted, want[1] / wanted] } else { [0.0, 0.0] };
    // To the right of the way it's going, with 0 yaw facing +Z and +X on its left: (−z, x).
    let right = [-heading[1], heading[0]];

    let mut lean = [0.0, 0.0];
    let mut held = 0.0;
    for &(other, there) in others {
        let dx = at.x - there.x;
        if dx >= apart || dx <= -apart || other == me || Some(other) == foe {
            continue;
        }
        let dz = at.z - there.z;
        let dy = at.y - there.y;
        let across = dx * dx + dz * dz;
        if across >= apart * apart || dy > STOREY || dy < -STOREY {
            continue;
        }
        let metres = across.sqrt();
        // The lower ID goes +X, whichever of the two is asking.
        let away = if metres < ON_TOP {
            [if me.raw() < other.raw() { 1.0 } else { -1.0 }, 0.0]
        } else {
            [dx / metres, dz / metres]
        };
        let closeness = 1.0 - metres / apart;
        lean[0] += away[0] * closeness * LEAN;
        lean[1] += away[1] * closeness * LEAN;

        let ahead = -(away[0] * heading[0] + away[1] * heading[1]);
        if ahead > 0.0 {
            let beside = away[0] * right[0] + away[1] * right[1];
            let side = if beside > 0.0 || (beside == 0.0 && me.raw() < other.raw()) { 1.0 } else { -1.0 };
            let step = side * (1.0 - beside.abs()) * ahead * closeness * LEAN;
            lean[0] += right[0] * step;
            lean[1] += right[1] * step;
            held += ahead * closeness;
        }
    }
    // Someone in the way takes the press out of its want, or the back of a crowd leans on the front.
    let held = (held * HOLD).min(1.0);
    lean[0] -= want[0] * held;
    lean[1] -= want[1] * held;
    lean
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: u32) -> ActorId {
        ActorId::from_raw(raw).expect("not zero")
    }

    fn at(x: f32, z: f32) -> Vec3 {
        Vec3::new(x, 0.0, z)
    }

    const APART: f32 = 1.2;

    fn lean(me: u32, here: Vec3, want: [f32; 2], others: &[(ActorId, Vec3)]) -> [f32; 2] {
        lean_on(id(me), None, here, APART, want, others)
    }

    #[test]
    fn nobody_near_leans_on_nobody() {
        let others = [(id(1), at(0.0, 0.0)), (id(2), at(APART, 0.0)), (id(3), at(0.0, -APART))];
        assert_eq!(lean(1, at(0.0, 0.0), [0.0, 0.7], &others), [0.0, 0.0], "it's not its own crowd");
        assert_eq!(lean(9, at(0.0, 3.0), [0.0, 0.7], &others), [0.0, 0.0], "they're all too far");
        let upstairs = [(id(2), Vec3::new(0.0, 3.0, 0.0))];
        assert_eq!(lean(1, at(0.0, 0.0), [0.0, 0.7], &upstairs), [0.0, 0.0], "that's another floor");
        let foe = [(id(2), at(0.5, 0.0))];
        assert_eq!(lean_on(id(1), Some(id(2)), at(0.0, 0.0), APART, [0.0, 0.7], &foe), [0.0, 0.0]);
        assert_eq!(lean_on(id(1), None, at(0.0, 0.0), 0.0, [0.0, 0.7], &foe), [0.0, 0.0], "apart is 0");
    }

    /// Whoever's asking, and in whichever order, two standing near each other are pushed exactly
    /// opposite ways, and three add up the same in any order.
    #[test]
    fn it_reads_the_same_whoever_asks_and_in_whatever_order() {
        let (a, b) = (at(0.25, 0.5), at(0.75, 0.0));
        let from_a = lean(1, a, [0.0, 0.0], &[(id(2), b)]);
        let from_b = lean(2, b, [0.0, 0.0], &[(id(1), a)]);
        assert_eq!(from_a, [-from_b[0], -from_b[1]]);

        let everyone = [(id(1), a), (id(2), b), (id(3), at(0.3, -0.25)), (id(4), at(-0.4, 0.5))];
        let want = [0.3, 0.6];
        let forward = lean(1, a, want, &everyone);
        let mut backward = everyone;
        backward.reverse();
        let backward = lean(1, a, want, &backward);
        assert!((forward[0] - backward[0]).abs() < 1e-6 && (forward[1] - backward[1]).abs() < 1e-6);
    }

    /// Exactly on top of each other is the one place with no direction in it: they still part, each its
    /// own way, and the same way however the list is ordered.
    #[test]
    fn two_on_top_of_each_other_part_by_their_ids() {
        let here = at(1.0, 1.0);
        let low = lean(1, here, [0.0, 0.0], &[(id(2), here)]);
        let high = lean(2, here, [0.0, 0.0], &[(id(1), here)]);
        assert!(low[0] > 1.0 && high[0] < -1.0, "{low:?} {high:?}");
        assert_eq!(low, [-high[0], -high[1]]);
    }

    /// What makes a file fan out: someone dead ahead moves you to the side, in a way a push can't.
    #[test]
    fn someone_dead_ahead_is_stepped_round() {
        let ahead = [(id(2), at(0.0, 0.8))];
        let step = lean(1, at(0.0, 0.0), [0.0, 1.0], &ahead);
        assert!(step[1] < 0.0, "it holds it back: {step:?}");
        assert!(step[0] < -0.2, "and to its right, which is −X walking +Z: {step:?}");
        let other_side = lean(3, at(0.0, 0.0), [0.0, 1.0], &ahead);
        assert_eq!(other_side[0], -step[0], "the higher of the pair goes the other way");

        // Already to one side, it goes on that way.
        let left_of_it = lean(1, at(0.1, 0.0), [0.0, 1.0], &ahead);
        let dead_ahead_of_nothing = lean(1, at(0.0, 0.0), [0.0, 1.0], &[(id(2), at(0.0, -0.8))]);
        assert!(left_of_it[0] > 0.0, "{left_of_it:?}");
        assert_eq!(dead_ahead_of_nothing[0], 0.0, "someone behind only pushes: {dead_ahead_of_nothing:?}");
    }

    /// A full row in front balances: nothing moves it to one side for ever.
    #[test]
    fn someone_either_side_ahead_balances() {
        let row = [(id(2), at(-0.5, 0.6)), (id(3), at(0.5, 0.6))];
        let step = lean(1, at(0.0, 0.0), [0.0, 1.0], &row);
        assert!(step[0].abs() < 1e-5, "{step:?}");
        assert!(step[1] < 0.0, "{step:?}");
    }

    #[test]
    fn a_stick_with_nobody_near_stays_to_the_bit_and_one_with_someone_goes_no_faster() {
        let mut crowd = Crowd::with_capacity(4);
        let stick = Stick::from_axes(0.3, 0.4);
        assert_eq!(crowd.lean(id(1), None, at(0.0, 0.0), APART, stick, 0.35), stick);

        crowd.add(id(2), at(-0.5, 0.0));
        let pushed = crowd.lean(id(1), None, at(0.0, 0.0), APART, stick, 0.35).to_vec2();
        assert!(pushed.x > 0.3, "it's pushed away: {pushed}");
        // No faster than it was going, which here is more than `most`, give or take a step of the stick.
        let speed = (pushed.x * pushed.x + pushed.y * pushed.y).sqrt();
        assert!(speed < 0.5 + 0.01, "{speed}");

        let standing = crowd.lean(id(1), None, at(0.0, 0.0), APART, Stick::CENTER, 0.35).to_vec2();
        let speed = (standing.x * standing.x + standing.y * standing.y).sqrt();
        assert!(speed > 0.3 && speed < 0.35 + 0.01, "it moves off at `most`: {speed}");

        crowd.clear();
        crowd.add(id(2), at(-APART + 0.01, 0.0));
        let barely = crowd.lean(id(1), None, at(0.0, 0.0), APART, Stick::CENTER, 0.35);
        assert_eq!(barely, Stick::CENTER, "that's too little to walk for");
    }
}
