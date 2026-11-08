//! Ground support as a graph settles from gait into a stationary stance.

use sapphire_math::Vec3;

use crate::pose_math::PoseMath;
use sapphire_math::ease::smoothstep;

pub(super) const TURN_CATCH_LIFT: f32 = 0.065;
pub(super) const TURN_RELEASE_CLEARANCE: f32 = 0.035;
const TURN_RELEASE_SECONDS: f32 = 3.0 / 60.0;
const GUARD_STEP_MIN_DISTANCE: f32 = 0.11;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SoleHistory {
    points: [Vec3; 2],
    ground: [f32; 2],
    movement: [f32; 2],
}

impl SoleHistory {
    pub(super) fn new(points: [Vec3; 2], ground: [f32; 2], previous: Option<Self>, dt: f32) -> Self {
        let movement = previous.map_or([0.0; 2], |previous| {
            std::array::from_fn(|pivot| {
                (points[pivot] - previous.points[pivot]).with_y(0.0).length() / (dt * 60.0).max(0.25)
            })
        });
        Self { points, ground, movement }
    }

    pub(super) fn grounded_point(self, pivot: usize, gap: f32) -> Option<Vec3> {
        let point = self.points[pivot];
        ((point.y - self.ground[pivot]).abs() <= gap).then_some(point)
    }

    pub(super) fn clearance(self) -> f32 {
        (self.points[0].y - self.ground[0]).min(self.points[1].y - self.ground[1])
    }

    pub(super) fn ball(self) -> Vec3 {
        self.points[1]
    }

    pub(super) fn point(self, pivot: usize) -> Vec3 {
        self.points[pivot]
    }

    pub(super) fn low_pivot(self) -> usize {
        usize::from(self.points[1].y - self.ground[1] < self.points[0].y - self.ground[0])
    }

    pub(super) fn support_cost(self) -> f32 {
        let pivot = self.low_pivot();
        2.5 * self.clearance().abs() + 0.35 * self.movement[pivot]
    }

    pub(super) fn touchdown(self) -> Option<(usize, Vec3)> {
        let (anchor, _) = self.best(true)?;
        // A catching shoe can still roll a little across its contact patch.
        (self.movement[anchor.pivot] <= 0.03).then_some((anchor.pivot, anchor.world))
    }

    fn best(self, must_touch: bool) -> Option<(Anchor, f32)> {
        let rank = |pivot: usize| {
            let gap = (self.points[pivot].y - self.ground[pivot]).abs();
            if must_touch && gap > 0.014 { f32::INFINITY } else { gap + self.movement[pivot] * 0.5 }
        };
        let pivot = usize::from(rank(1) < rank(0));
        let score = rank(pivot);
        let mut world = self.points[pivot];
        if must_touch {
            world.y = self.ground[pivot];
        }
        score.is_finite().then_some((Anchor { pivot, world }, score))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Anchor {
    pivot: usize,
    world: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Gather {
    anchor: [Option<Anchor>; 2],
    supporting: usize,
    phase: f32,
    weight: f32,
    returning: bool,
    turning: Option<TurnStep>,
    guard_step: Option<GuardStep>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GuardStep {
    /// Where the previously loaded shoe actually was, after the reach solve.
    takeoff: Anchor,
    /// The first gathering foot has caught and will now carry the weight.
    load: Anchor,
    seconds: f32,
    elapsed: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TurnStep {
    lifts_from_ground: bool,
    in_flight: bool,
    guard_swing: Option<GuardSwing>,
    caught: Option<Anchor>,
    departure: Option<(f32, Vec3)>,
    departing_shoe: Option<DepartingShoe>,
    retain_catch: bool,
    reach_release: Option<TurnRelease>,
    applied_catch: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct GuardSwing {
    displayed: Anchor,
    began: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct DepartingShoe {
    pivot: usize,
    elapsed: f32,
    arrived: bool,
}

#[derive(Clone, Copy)]
pub(super) struct DepartureSpace {
    pub outward: Vec3,
    pub opposite_ball: Option<Vec3>,
    pub scale: f32,
    pub dt: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct TurnRelease {
    offset: Vec3,
    elapsed: f32,
    interrupt_pivot: Option<usize>,
    /// Model-space reach compensation from the first toe-off, for a same-clock re-solve.
    first_shift: Option<Vec3>,
}

type History<'a, F> = crate::graph::canonical::Canonical<'a, F>;

impl SoleHistory {
    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        for point in self.points {
            out.vec3(point);
        }
        out.floats(&self.ground);
        out.floats(&self.movement);
    }
}

impl Anchor {
    fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.len(self.pivot);
        out.vec3(self.world);
    }
}

impl Gather {
    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        for anchor in self.anchor {
            out.optional(anchor, |out, anchor| anchor.write_canonical(out));
        }
        out.len(self.supporting);
        out.floats(&[self.phase, self.weight]);
        out.flag(self.returning);
        out.optional(self.guard_step, |out, guard| {
            guard.takeoff.write_canonical(out);
            guard.load.write_canonical(out);
            out.floats(&[guard.seconds, guard.elapsed]);
        });
        out.optional(self.turning, |out, turn| {
            out.flag(turn.lifts_from_ground);
            out.flag(turn.in_flight);
            out.optional(turn.guard_swing, |out, guard| {
                guard.displayed.write_canonical(out);
                out.f32(guard.began);
            });
            out.optional(turn.caught, |out, anchor| anchor.write_canonical(out));
            out.optional(turn.departure, |out, (phase, point)| {
                out.f32(phase);
                out.vec3(point);
            });
            out.optional(turn.departing_shoe, |out, shoe| {
                out.len(shoe.pivot);
                out.f32(shoe.elapsed);
                out.flag(shoe.arrived);
            });
            out.flag(turn.retain_catch);
            out.optional(turn.reach_release, |out, release| {
                out.vec3(release.offset);
                out.f32(release.elapsed);
                out.optional(release.interrupt_pivot, History::len);
                out.optional(release.first_shift, History::vec3);
            });
            out.vec3(turn.applied_catch);
        });
    }
}
impl Gather {
    pub(super) fn begin(known: [Option<SoleHistory>; 2], phase: f32) -> Option<Self> {
        let sides = known.map(|sole| sole.and_then(|sole| sole.best(true)));
        let supporting = usize::from(
            sides[1].map_or(f32::INFINITY, |(_, score)| score)
                < sides[0].map_or(f32::INFINITY, |(_, score)| score),
        );
        let mut anchor = known.map(|sole| sole.and_then(|sole| sole.best(false)).map(|(point, _)| point));
        anchor[supporting] = Some(sides[supporting]?.0);
        Some(Self {
            anchor,
            supporting,
            phase,
            weight: 1.0,
            returning: false,
            turning: None,
            guard_step: None,
        })
    }

    pub(super) fn begin_turn(
        known: [Option<SoleHistory>; 2],
        phase: f32,
        outgoing: Option<Self>,
    ) -> Option<Self> {
        let mut step = Self::begin(known, phase)?;
        if let Some(side) = outgoing.filter(|old| !old.is_turning()).map(Self::loaded_side)
            && let Some((anchor, _)) = known[side].and_then(|sole| sole.best(true))
        {
            step.supporting = side;
            step.anchor[side] = Some(anchor);
        }
        let free = known[1 - step.supporting];
        let guard_swing = outgoing
            .and_then(|old| old.guard_step)
            .filter(|step| step.elapsed < step.seconds)
            .and(free)
            .filter(|sole| sole.clearance() > 0.025)
            .and_then(|sole| sole.best(false))
            .map(|(displayed, _)| GuardSwing { displayed, began: phase });
        step.turning = Some(TurnStep {
            lifts_from_ground: free.is_some_and(|sole| sole.clearance() <= 0.025),
            in_flight: free.is_some_and(|sole| sole.clearance() > 0.03),
            guard_swing,
            caught: None,
            departure: None,
            departing_shoe: None,
            retain_catch: false,
            reach_release: None,
            applied_catch: Vec3::ZERO,
        });
        Some(step)
    }

    pub(super) fn advance(&mut self, phase: Option<f32>, dt: f32) -> bool {
        if let Some(shoe) = self.turning.as_mut().and_then(|turn| turn.departing_shoe.as_mut()) {
            shoe.elapsed += dt;
        }
        if let Some(turn) = &mut self.turning
            && let Some(release) = &mut turn.reach_release
        {
            release.elapsed += dt;
            if release.elapsed >= TURN_RELEASE_SECONDS {
                turn.reach_release = None;
            }
        }
        if let Some(phase) = phase {
            self.phase = phase.clamp(0.0, if self.is_turning() { 1.45 } else { 1.0 });
            self.weight = 1.0;
            self.returning = false;
            if let Some(step) = &mut self.guard_step {
                step.elapsed = (step.elapsed + dt).min(step.seconds);
            }
        } else if self.returning {
            self.weight = (self.weight - dt * 15.0).max(0.0);
        } else {
            // The first sample of the next movement inherits the displayed foot.
            self.returning = true;
        }
        self.weight > 0.0 || self.retains_turn_catch(1 - self.supporting) || self.has_departing_shoe()
    }

    pub(super) fn is_returning(&self) -> bool {
        self.returning
    }

    pub(super) fn is_turning(&self) -> bool {
        self.turning.is_some()
    }

    pub(super) fn turn_support(&self) -> Option<usize> {
        (self.is_turning() && !self.returning).then_some(self.supporting)
    }

    fn loaded_side(self) -> usize {
        if self.guard_step.is_some() { 1 - self.supporting } else { self.supporting }
    }

    /// After the first recovery reaches guard, see if the original plant left the stance long.
    pub(super) fn guard_support_to_check(self) -> Option<usize> {
        (self.phase >= 0.9 && !self.returning && !self.is_turning() && self.guard_step.is_none())
            .then_some(self.supporting)
    }

    pub(super) fn begin_guard_step(
        &mut self,
        target: [Vec3; 2],
        solved: [Option<SoleHistory>; 2],
        scale: f32,
    ) {
        let (Some(anchor), Some(sole)) = (self.anchor[self.supporting], solved[self.supporting]) else {
            return;
        };
        let takeoff = Anchor { pivot: anchor.pivot, world: sole.points[anchor.pivot] };
        let distance = (target[takeoff.pivot] - takeoff.world).with_y(0.0).length() / scale;
        if distance <= GUARD_STEP_MIN_DISTANCE {
            return;
        }
        if let Some((pivot, world)) = solved[1 - self.supporting].and_then(SoleHistory::touchdown) {
            // A short recovery takes a quick step. A long one has time to clear, travel and catch.
            let seconds = (7.5 + 10.0 * distance).clamp(9.0, 13.0) / 60.0;
            self.guard_step =
                Some(GuardStep { takeoff, load: Anchor { pivot, world }, seconds, elapsed: 0.0 });
        }
    }

    pub(super) fn retain_turn_catch(&mut self, grounded_gait: bool, dt: f32) {
        if let Some(turn) = &mut self.turning {
            if turn.retain_catch
                && !grounded_gait
                && let Some(catch) = turn.caught.take()
            {
                // Preserve the last solved planar correction for the first frame of a new
                // route, even if the original turn's return has already elapsed.
                turn.reach_release = Some(TurnRelease {
                    offset: turn.applied_catch,
                    elapsed: -dt,
                    interrupt_pivot: Some(catch.pivot),
                    first_shift: None,
                });
            }
            turn.retain_catch = grounded_gait;
        }
    }

    pub(super) fn retains_turn_catch(self, side: usize) -> bool {
        side != self.supporting
            && self.turning.is_some_and(|turn| {
                (turn.retain_catch && turn.caught.is_some()) || turn.reach_release.is_some()
            })
    }

    pub(super) fn holds_turn_catch(self, side: usize) -> bool {
        self.retains_turn_catch(side) && self.turning.is_some_and(|turn| turn.caught.is_some())
    }

    pub(super) fn affects(self, side: usize) -> bool {
        self.weight > 0.0
            || self.retains_turn_catch(side)
            || (side == self.supporting && self.has_departing_shoe())
    }

    fn has_departing_shoe(self) -> bool {
        self.turning.and_then(|turn| turn.departing_shoe).is_some_and(|shoe| !shoe.arrived)
    }

    pub(super) fn stop_departing_shoe(&mut self, side: usize) {
        if side == self.supporting
            && let Some(shoe) = self.turning.as_mut().and_then(|turn| turn.departing_shoe.as_mut())
        {
            shoe.arrived = true;
        }
    }

    pub(super) fn release_turn_catch(&mut self) {
        if let Some(turn) = &mut self.turning {
            turn.caught = None;
            turn.reach_release = None;
        }
    }

    pub(super) fn begin_reach_release(&mut self, offset: Vec3, pelvis_shift: Vec3) {
        if let Some(turn) = &mut self.turning {
            turn.caught = None;
            turn.reach_release = Some(TurnRelease {
                offset,
                elapsed: 0.0,
                interrupt_pivot: None,
                first_shift: Some(pelvis_shift),
            });
        }
    }

    pub(super) fn rebase_turn_interrupt(
        &mut self,
        side: usize,
        world: [Vec3; 2],
        solved: Option<SoleHistory>,
    ) {
        if side != self.supporting
            && let Some(release) = self.turning.as_mut().and_then(|turn| turn.reach_release.as_mut())
            && let Some(pivot) = release.interrupt_pivot.take()
            && let Some(point) = solved.and_then(|sole| sole.grounded_point(pivot, 0.025))
        {
            // The new move can already change the sampled ankle on its first frame.
            release.offset = (point - world[pivot]).with_y(0.0);
        }
    }

    pub(super) fn reach_release_start_shift(self, side: usize) -> Option<Vec3> {
        let release = self.turning?.reach_release?;
        (side != self.supporting && release.elapsed == 0.0).then_some(release.first_shift).flatten()
    }

    pub(super) fn remember_catch_offset(&mut self, offset: Vec3) {
        if let Some(turn) = &mut self.turning {
            turn.applied_catch = offset.with_y(0.0);
        }
    }

    pub(super) fn forefoot_weight(self, side: usize) -> f32 {
        let Some(turn) = self.turning else { return 0.0 };
        let anchor = if side == self.supporting { self.anchor[side] } else { turn.caught };
        if anchor.is_some_and(|anchor| anchor.pivot == 1) { self.weight_on(side) } else { 0.0 }
    }

    pub(super) fn catch_turn_step(&mut self, solved: [Option<SoleHistory>; 2]) {
        let Some(turn) = &mut self.turning else { return };
        let Some(sole) = solved[1 - self.supporting] else { return };
        turn.in_flight |= sole.clearance() > 0.03;
        if turn.in_flight && self.phase >= 0.7 && turn.caught.is_none() {
            turn.caught = sole.best(true).map(|(anchor, _)| anchor);
        }
    }

    pub(super) fn begin_turn_departure(
        &mut self,
        side: usize,
        world: [Vec3; 2],
        lift: f32,
        solved: Option<SoleHistory>,
    ) {
        if side == self.supporting
            && self.phase >= 1.0
            && let Some(turn) = &mut self.turning
            && turn.departure.is_none()
            && (turn.caught.is_some() || lift > 0.035 || self.phase >= 1.2)
            && let Some(anchor) = self.anchor[side]
        {
            turn.departure = Some((self.phase, anchor.world - world[anchor.pivot]));
            if turn.guard_swing.is_some() && turn.caught.is_some() && solved.is_some() {
                turn.departing_shoe =
                    Some(DepartingShoe { pivot: anchor.pivot, elapsed: 0.0, arrived: false });
            }
        }
    }

    /// A restart is still rotating when its first step catches. Start the outgoing shoe from
    /// its displayed position and keep it clear while it closes on the moving gait target.
    pub(super) fn departing_correction(
        &mut self,
        side: usize,
        world: [Vec3; 2],
        ground_shift: f32,
        solved: Option<SoleHistory>,
        space: DepartureSpace,
    ) -> Option<Vec3> {
        if side != self.supporting {
            return None;
        }
        let shoe = self.turning.as_mut()?.departing_shoe.as_mut()?;
        if shoe.arrived {
            return Some(Vec3::ZERO);
        }
        let Some(solved) = solved else {
            shoe.arrived = true;
            return Some(Vec3::ZERO);
        };
        let from = solved.points[shoe.pivot];
        let to = world[shoe.pivot] + Vec3::Y * ground_shift;
        let DepartureSpace { outward, opposite_ball, scale, dt } = space;
        let planar = (to - from).with_y(0.0);
        // Over the first three ticks, let the shoe gather velocity after its loaded pivot.
        let lift_off = smoothstep(0.0, 0.05, shoe.elapsed);
        let budget = (2.5 + 9.5 * lift_off) * dt * scale;
        let travel = planar.clamp_length_max(budget);
        let travel = if let Some(opposite) = opposite_ball {
            // Keep the swing on its own side while passing the just-loaded shoe. Farther ahead
            // or behind, the authored target determines the width again.
            let gap = (from + travel - opposite).with_y(0.0);
            let lateral = gap.dot(outward);
            let along = (gap - outward * lateral).length();
            let close = 1.0 - smoothstep(0.17 * scale, 0.36 * scale, along);
            let steer = (0.24 * scale - lateral).max(0.0) * close;
            let across = travel.dot(outward);
            let routed = (across + steer).clamp(-budget, budget);
            let forward_budget = (budget * budget - routed * routed).max(0.0).sqrt();
            outward * routed + (travel - outward * across).clamp_length_max(forward_budget)
        } else {
            travel
        };
        let left = (planar - travel).length();
        let clearance = 0.075 * scale * smoothstep(0.0, 0.20 * scale, left);
        let y = to.y.max(solved.ground[shoe.pivot] + clearance);
        let vertical = (y - from.y).clamp(-2.4 * dt * scale, 2.4 * dt * scale);
        let correction = from + travel + Vec3::Y * vertical - to;
        if dt > 0.0 && correction.length_squared() < 1e-8 {
            shoe.arrived = true;
        }
        Some(correction)
    }

    pub(super) fn correction(
        self,
        side: usize,
        world: [Vec3; 2],
        ground_shift: f32,
        lift: f32,
        math: PoseMath,
    ) -> Vec3 {
        if let Some(step) = self.guard_step {
            if side != self.supporting {
                let difference = step.load.world - world[step.load.pivot];
                return difference.with_y(difference.y - ground_shift) * self.weight;
            }
            let difference = step.takeoff.world - world[step.takeoff.pivot];
            let progress = step.elapsed / step.seconds;
            let carry = 1.0 - smoothstep(0.12, 0.98, progress);
            let clearance =
                (0.065 * math.sin(std::f32::consts::PI * progress).max(0.0) - lift.max(0.0)).max(0.0);
            return ((difference.with_y(difference.y - ground_shift) * carry) + Vec3::Y * clearance)
                * self.weight;
        }
        let Some(anchor) = self.anchor[side] else { return Vec3::ZERO };
        let difference = anchor.world - world[anchor.pivot];
        if side == self.supporting {
            if let Some((started, offset)) = self.turning.and_then(|turn| turn.departure) {
                let progress = ((self.phase - started) / (1.45 - started).max(0.1)).clamp(0.0, 1.0);
                let release = 1.0 - smoothstep(0.0, 1.0, progress);
                let clearance = (0.06 * math.sin(std::f32::consts::PI * progress) - lift.max(0.0)).max(0.0);
                return (offset.with_y(difference.y - ground_shift) * release + Vec3::Y * clearance)
                    * self.weight;
            }
            return difference.with_y(difference.y - ground_shift) * self.support_weight();
        }
        if let Some(turn) = self.turning {
            if let Some(release) = turn.reach_release {
                let remaining = release_weight(release.elapsed);
                let clearance = if turn.retain_catch {
                    if release.elapsed == 0.0 {
                        release.offset.y
                    } else {
                        (TURN_RELEASE_CLEARANCE - lift.max(0.0)).clamp(0.0, release.offset.y.max(0.0))
                    }
                } else {
                    0.0
                };
                return release.offset.with_y(clearance) * remaining;
            }
            if let Some(catch) = turn.caught {
                let difference = catch.world - world[catch.pivot];
                if turn.retain_catch {
                    // Gait has taken over, but the new plant is still bearing weight. Keep its
                    // pivot until the gait lifts it, while giving the authored toe-off its height.
                    let planar = 1.0 - smoothstep(0.01, TURN_CATCH_LIFT, lift);
                    let height = 1.0 - smoothstep(0.0, 0.012, lift);
                    return (difference * planar).with_y((difference.y - ground_shift) * height);
                }
                let peel = smoothstep(0.025, 0.060, lift);
                return difference.with_y(difference.y - ground_shift) * (1.0 - peel) * self.weight;
            }
            if let Some(guard) = turn.guard_swing {
                // A new steering target can sample a grounded shoe immediately. Resume from
                // the displayed recovery shoe and keep the transfer clear of the floor.
                let progress = ((self.phase - guard.began) / 0.68).clamp(0.0, 1.0);
                let carry = 1.0 - smoothstep(0.0, 1.0, progress);
                let difference = guard.displayed.world - world[guard.displayed.pivot];
                let arc = math.sin(std::f32::consts::PI * progress).max(0.0);
                let clearance = (0.036 - lift.max(0.0)).max(0.0) * arc;
                return (difference.with_y(difference.y - ground_shift) * carry + Vec3::Y * clearance)
                    * self.weight;
            }
            // An already airborne shoe is taking its next step. A planted shoe must first
            // clear the ground before it can travel with the turning body.
            if !turn.lifts_from_ground {
                return Vec3::ZERO;
            }
            let swing = smoothstep(0.12, 0.98, self.phase);
            let arch = math.sin(std::f32::consts::PI * self.phase.clamp(0.0, 1.0));
            let clearance = (0.068 * arch - lift.max(0.0)).max(0.0);
            return (difference * (1.0 - swing)).with_y(clearance) * self.weight;
        }
        let swing = smoothstep(0.0, 0.85, self.phase);
        let clearance = (0.05 - lift.max(0.0)).max(0.0) * math.sin(swing * std::f32::consts::PI);
        (difference * (1.0 - swing)).with_y(clearance) * self.weight
    }

    /// The hips can settle a little over the supporting sole if the guard would rise out of reach.
    pub(super) fn pelvis_room(self, side: usize) -> f32 {
        0.025 * self.weight_on(side)
    }

    pub(super) fn pelvis_shift(self, side: usize, toward: Vec3, reach: f32, scale: f32) -> Vec3 {
        let horizontal = toward.with_y(0.0);
        let available = (reach * reach - toward.y * toward.y).max(0.0).sqrt();
        let deficit = (horizontal.length() - available).max(0.0);
        let room = 0.10 * self.weight_on(side) / scale;
        horizontal.normalize_or_zero() * deficit.min(room)
    }

    fn weight_on(self, side: usize) -> f32 {
        if let Some(step) = self.guard_step {
            // Keep the original leg's reach as its heel first rises. Passing the hip correction
            // immediately would retract a still-loaded shoe, then extend it again in the swing.
            let lift = smoothstep(0.16, 0.43, step.elapsed / step.seconds);
            let load = if side == self.supporting { 1.0 - lift } else { f32::from(lift >= 1.0) };
            self.weight * load
        } else if let Some(turn) = self.turning.filter(|turn| {
            (turn.caught.is_some() || turn.reach_release.is_some()) && turn.departure.is_some()
        }) {
            let weight =
                turn.reach_release.map_or(if turn.retain_catch { 1.0 } else { self.weight }, |release| {
                    release_weight(release.elapsed)
                });
            weight * f32::from(side != self.supporting)
        } else {
            self.support_weight() * f32::from(side == self.supporting)
        }
    }

    fn support_weight(self) -> f32 {
        let transferred = if self.is_turning() { smoothstep(1.0, 1.45, self.phase) } else { 0.0 };
        self.weight * (1.0 - transferred)
    }
}

fn release_weight(elapsed: f32) -> f32 {
    1.0 - smoothstep(0.0, TURN_RELEASE_SECONDS, elapsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grounded_sole_holds_while_the_other_clears_then_sets_down() {
        let supported = [Vec3::X * 0.1, Vec3::new(0.1, 0.01, 0.14)];
        let released = [Vec3::new(-0.1, 0.025, -0.25), Vec3::new(-0.1, 0.06, -0.11)];
        let histories =
            [supported, released].map(|points| Some(SoleHistory::new(points, [0.0; 2], None, 0.02)));
        let mut gather = Gather::begin(histories, 0.0).expect("support");
        let guard = [Vec3::new(0.13, 0.0, 0.08), Vec3::new(0.13, 0.0, 0.22)];
        for phase in [0.0, 0.5, 1.0] {
            assert!(gather.advance(Some(phase), 0.02));
            let support = gather.correction(0, guard, 0.0, 0.0, PoseMath::Legacy);
            assert!((guard[0] + support).abs_diff_eq(supported[0], 1e-6));
            let free = gather.correction(1, guard, 0.0, 0.0, PoseMath::Legacy);
            match phase {
                0.0 => assert!((guard[0] + free).with_y(released[0].y).abs_diff_eq(released[0], 1e-6)),
                0.5 => assert!(free.y > 0.03),
                _ => assert!(free.abs_diff_eq(Vec3::ZERO, 1e-6)),
            }
        }
        assert!(gather.advance(None, 0.02));
        assert!(
            (guard[0] + gather.correction(0, guard, 0.0, 0.0, PoseMath::Legacy))
                .abs_diff_eq(supported[0], 1e-6)
        );
        assert!(gather.advance(None, 0.02));
        let fading = gather.correction(0, guard, 0.0, 0.0, PoseMath::Legacy);
        assert!((0.0..guard[0].distance(supported[0])).contains(&fading.length()));
        assert!(!gather.advance(None, 0.08));
    }

    #[test]
    fn either_foot_can_be_the_support() {
        let higher = [Vec3::new(0.1, 0.04, 0.02), Vec3::new(0.1, 0.05, 0.16)];
        let lower = [Vec3::new(-0.1, 0.01, -0.02), Vec3::new(-0.1, 0.0, 0.12)];
        for (support, points) in [[lower, higher], [higher, lower]].into_iter().enumerate() {
            let history = points.map(|points| Some(SoleHistory::new(points, [0.0; 2], None, 0.02)));
            let gather = Gather::begin(history, 0.25).expect("support");
            assert_eq!(gather.supporting, support);
            assert_eq!(gather.anchor[support].expect("anchor").pivot, 1);
        }
    }

    #[test]
    fn a_long_stop_takes_one_more_step_after_the_free_foot_catches() {
        let sole = |x, y, z| [Vec3::new(x, y, z), Vec3::new(x, y, z + 0.15)];
        let history = |points| Some(SoleHistory::new(points, [0.0; 2], None, 1.0 / 60.0));
        let extended = sole(0.1, 0.0, 0.39);
        let guard = [sole(0.1, 0.0, 0.08), sole(-0.1, 0.0, -0.12)];
        let lifted = sole(-0.1, 0.08, -0.20);
        let mut gather = Gather::begin([history(extended), history(lifted)], 0.0).expect("first plant");
        assert_eq!(gather.guard_support_to_check(), None);
        assert!(gather.advance(Some(1.0), 1.0 / 60.0));
        gather.begin_guard_step(guard[0], [history(extended), history(lifted)], 1.0);
        assert!(gather.guard_step.is_none(), "transfer waits for real contact");
        gather.begin_guard_step(extended, [history(extended), history(guard[1])], 1.0);
        assert!(gather.guard_step.is_none(), "already compact");
        // The IK reach limit may already have brought the original foot closer than its anchor.
        let displayed = extended.map(|point| point - Vec3::Z * 0.045);
        gather.begin_guard_step(guard[0], [history(displayed), history(guard[1])], 1.0);
        assert_eq!(gather.loaded_side(), 1);
        assert!(
            (guard[0][0] + gather.correction(0, guard[0], 0.0, 0.0, PoseMath::Legacy))
                .abs_diff_eq(displayed[0], 1e-6)
        );
        let seconds = gather.guard_step.unwrap().seconds;
        assert!(gather.advance(Some(1.0), seconds * 0.5));
        assert!(gather.correction(0, guard[0], 0.0, 0.0, PoseMath::Legacy).y > 0.06);
        let moved = guard[1].map(|point| point + Vec3::Z * 0.03);
        assert!(
            (moved[0] + gather.correction(1, moved, 0.0, 0.0, PoseMath::Legacy))
                .abs_diff_eq(guard[1][0], 1e-6)
        );
        assert!(gather.advance(Some(1.0), seconds * 0.5));
        assert!(gather.correction(0, guard[0], 0.0, 0.0, PoseMath::Legacy).abs_diff_eq(Vec3::ZERO, 1e-6));
        assert_eq!(gather.guard_support_to_check(), None, "only one transfer");
    }

    #[test]
    fn a_guard_step_uses_the_owner_clock_and_hands_the_caught_foot_to_a_new_turn() {
        let sole = |x, y, z| [Vec3::new(x, y, z), Vec3::new(x, y, z + 0.15)];
        let history = |points| Some(SoleHistory::new(points, [0.0; 2], None, 1.0 / 60.0));
        let extended = sole(0.1, 0.0, 0.40);
        let guard = [sole(0.1, 0.0, 0.04), sole(-0.1, 0.0, -0.12)];
        let mut gather = Gather::begin([history(extended), None], 1.0).expect("first plant");
        gather.begin_guard_step(guard[0], [history(extended), history(guard[1])], 1.0);
        let seconds = gather.guard_step.unwrap().seconds;

        let mut whole = gather;
        assert!(whole.advance(Some(1.0), seconds * 0.48));
        for dt in [seconds * 0.19, 0.0, seconds * 0.29] {
            assert!(gather.advance(Some(1.0), dt));
        }
        let displayed = guard[0];
        let correction = |step: Gather| step.correction(0, displayed, 0.0, 0.0, PoseMath::Legacy);
        assert!(correction(whole).abs_diff_eq(correction(gather), 1e-6));

        let current = displayed.map(|point| point + correction(gather));
        assert!(current[0].y > 0.05);
        assert!(gather.advance(None, 1.0 / 60.0));
        assert!(correction(whole).abs_diff_eq(correction(gather), 1e-6), "first resume keeps the shown pose");
        assert!(gather.advance(None, 1.0 / 60.0));
        assert!(correction(gather).length() < correction(whole).length(), "the old path gives way to input");
        let mut turn = Gather::begin_turn([history(current), history(guard[1])], 0.11, Some(gather))
            .expect("the receiving foot has touched down");
        assert_eq!(turn.turn_support(), Some(1));
        let drift = guard[1].map(|point| point + Vec3::Z * 0.025);
        assert!(
            (drift[0] + turn.correction(1, drift, 0.0, 0.0, PoseMath::Legacy)).abs_diff_eq(guard[1][0], 1e-6)
        );
        assert!(
            (displayed[0] + turn.correction(0, displayed, 0.0, 0.0, PoseMath::Legacy))
                .abs_diff_eq(current[0], 1e-6)
        );
        assert!(turn.advance(Some(0.45), 3.0 / 60.0));
        assert!(
            turn.correction(0, displayed, 0.0, 0.0, PoseMath::Legacy).y > 0.055,
            "keep the re-aimed shoe clear"
        );
        assert!(turn.advance(Some(0.80), 3.0 / 60.0));
        assert!(turn.correction(0, displayed, 0.0, 0.0, PoseMath::Legacy).abs_diff_eq(Vec3::ZERO, 1e-6));
    }

    #[test]
    fn a_restart_departure_starts_from_the_solved_shoe_and_survives_the_turn_timer() {
        let pair = |p| [p, p + Vec3::Z * 0.15];
        let history = |p| Some(SoleHistory::new(pair(p), [0.0; 2], None, 1.0 / 60.0));
        let drawn = Vec3::X * 0.1;
        let mut step =
            Gather::begin_turn([history(drawn), history(Vec3::NEG_X * 0.1)], 1.0, None).expect("both shoes");
        assert_eq!(step.supporting, 0);
        let turn = step.turning.as_mut().unwrap();
        let catch = Anchor { pivot: 0, world: Vec3::NEG_X * 0.1 };
        turn.guard_swing = Some(GuardSwing { displayed: catch, began: 0.1 });
        turn.caught = Some(catch);
        let target = pair(drawn + Vec3::Z * 0.8);
        let dt = 1.0 / 60.0;
        step.begin_turn_departure(0, target, 0.0, history(drawn));
        let space = |dt| DepartureSpace { outward: Vec3::X, opposite_ball: None, scale: 1.0, dt };
        let frozen = step.departing_correction(0, target, 0.0, history(drawn), space(0.0)).unwrap();
        let crossing = pair(drawn - Vec3::X * 0.1);
        let opposite_ball = Some(drawn - Vec3::X * 0.12);
        let steer = DepartureSpace { opposite_ball, ..space(dt) };
        let routed =
            crossing[0] + step.departing_correction(0, crossing, 0.0, history(drawn), steer).unwrap();
        assert!(routed.x >= drawn.x, "route outside the opposite shoe");
        assert!((target[0] + frozen).abs_diff_eq(drawn, 1e-6));
        let mut rendered = drawn;
        for frame in 0..18 {
            assert!(step.advance(None, dt) || !step.has_departing_shoe());
            let corrected =
                target[0] + step.departing_correction(0, target, 0.0, history(rendered), space(dt)).unwrap();
            assert!(corrected.z - rendered.z <= 0.201, "frame {frame}");
            if corrected.z < target[0].z - 0.15 {
                assert!(corrected.y > 0.025, "no grounded drag");
            }
            rendered = corrected;
        }
        assert!(rendered.abs_diff_eq(target[0], 1e-6));
        assert!(!step.has_departing_shoe());
    }

    #[test]
    fn releasing_during_flight_waits_for_a_real_sole_contact() {
        let airborne = [Vec3::Y * 0.04, Vec3::new(0.0, 0.035, 0.14)];
        let landing = [Vec3::new(-0.1, 0.007, -0.02), Vec3::new(-0.1, 0.02, 0.12)];
        let history = [airborne; 2].map(|points| Some(SoleHistory::new(points, [0.0; 2], None, 0.02)));
        assert!(Gather::begin(history, 0.0).is_none());
        let history = [airborne, landing].map(|points| Some(SoleHistory::new(points, [0.0; 2], None, 0.02)));
        let gather = Gather::begin(history, 0.35).expect("one foot is down");
        assert_eq!(gather.supporting, 1);
        let world = landing[0] + gather.correction(1, landing, 0.0, landing[0].y, PoseMath::Legacy);
        assert!(world.abs_diff_eq(landing[0].with_y(0.0), 1e-6));
    }

    #[test]
    fn a_turn_holds_the_loaded_foot_then_catches_the_swing_foot() {
        let support = [Vec3::new(0.13, 0.0, 0.08), Vec3::new(0.13, 0.0, 0.22)];
        let free = [Vec3::new(-0.12, 0.0, -0.08), Vec3::new(-0.12, 0.0, 0.06)];
        let sole = |points| Some(SoleHistory::new(points, [0.0; 2], None, 1.0 / 60.0));
        let mut step = Gather::begin_turn([sole(support), sole(free)], 0.2, None).expect("grounded turn");
        let moved = free.map(|point| point + Vec3::Z * 0.12);
        let lift = step.correction(1, moved, 0.0, 0.0, PoseMath::Legacy);
        assert!(lift.y > 0.03 && lift.z < -0.11, "the foot lifts before the swing travels");
        assert!(step.advance(Some(0.8), 1.0 / 60.0));
        step.catch_turn_step([sole(support), sole(free)]);
        assert!(step.turning.unwrap().caught.is_none(), "a foot must leave the floor before it can catch");
        step.catch_turn_step([sole(support), sole(moved.map(|point| point + Vec3::Y * 0.05))]);
        assert!(step.advance(Some(1.1), 1.0 / 60.0));
        step.catch_turn_step([sole(support), sole(moved)]);
        let goal = moved.map(|point| point + Vec3::Z * 0.06);
        assert!((goal[0] + step.correction(1, goal, 0.0, 0.0, PoseMath::Legacy)).abs_diff_eq(moved[0], 1e-6));
        assert_eq!(step.correction(1, goal, 0.0, 0.061, PoseMath::Legacy), Vec3::ZERO);

        let dt = 1.0 / 60.0;
        assert!(step.advance(None, dt));
        assert!(step.advance(None, dt));
        step.retain_turn_catch(true, dt);
        assert!((goal[0] + step.correction(1, goal, 0.0, 0.0, PoseMath::Legacy)).abs_diff_eq(moved[0], 1e-6));
        let lifted = goal.map(|point| point + Vec3::Y * 0.035);
        let peel = step.correction(1, lifted, 0.0, 0.035, PoseMath::Legacy);
        assert_eq!(peel.y, 0.0, "the toe-off keeps its authored lift");
        assert!(peel.z < 0.0 && peel.z > -0.06, "the lifted shoe begins to travel");
        assert_eq!(step.correction(1, lifted, 0.0, 0.066, PoseMath::Legacy), Vec3::ZERO);
        step.remember_catch_offset(Vec3::NEG_Z * 0.06);
        let mut interrupted = step;
        interrupted.retain_turn_catch(false, dt);
        assert!(interrupted.advance(None, dt));
        let action_goal = goal.map(|point| point + Vec3::X * 0.04);
        interrupted.rebase_turn_interrupt(1, action_goal, sole(moved));
        let first = interrupted.correction(1, action_goal, 0.0, 0.0, PoseMath::Legacy);
        assert!((action_goal[0] + first).abs_diff_eq(moved[0], 1e-6));

        assert!(step.advance(None, dt * 4.0));
        assert!(!step.affects(0));
        assert!(step.affects(1));
        assert!((goal[0] + step.correction(1, goal, 0.0, 0.0, PoseMath::Legacy)).abs_diff_eq(moved[0], 1e-6));
        let mut interrupted = step;
        interrupted.retain_turn_catch(false, dt);
        assert!(interrupted.advance(None, dt));
        assert_eq!(interrupted.correction(1, goal, 0.0, 0.0, PoseMath::Legacy), Vec3::NEG_Z * 0.06);
        assert!(interrupted.advance(None, dt));
        assert!(interrupted.correction(1, goal, 0.0, 0.0, PoseMath::Legacy).z > -0.06);
        assert!(!interrupted.advance(None, dt * 3.0));
        step.begin_reach_release(Vec3::new(0.0, 0.012, -0.06), Vec3::X * 0.02);
        let departing = step.correction(1, goal, 0.0, 0.0, PoseMath::Legacy);
        assert_eq!(departing, Vec3::new(0.0, 0.012, -0.06));
        assert_eq!(step.reach_release_start_shift(1), Some(Vec3::X * 0.02));
        assert!(step.advance(None, dt));
        assert_eq!(step.reach_release_start_shift(1), None);
        let next = step.correction(1, goal, 0.0, 0.0, PoseMath::Legacy);
        assert!(next.y > 0.0 && next.y < departing.y && next.z > departing.z);
        assert!(!step.advance(None, dt * 3.0));
    }
}
