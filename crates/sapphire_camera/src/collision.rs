//! The arm from the player to the eye, and the rule that the eye never enters a collider.
//!
//! Two things happen each frame, and it's the second that promises no clip:
//!
//! 1. **The arm decides where the eye should be.** A sphere cast along the line from the player's
//!    chest to where the rig wants the eye says what's in the way, and a second cast along where that
//!    line will be `lead_time` from now says what's coming, so the arm comes in *before* it arrives
//!    and pulling in is a spring rather than a kink. Coming in is quick and going out is slow, after a
//!    delay, so it doesn't pump.
//! 2. **The eye moves there by sweeping** from last frame's eye, sliding along anything it meets. It
//!    can't pass through what a sweep sees, so the arm's spring is free to lag, and a surface the arm
//!    reaches late only gets slid along.
//!
//! **A thin pillar** is the case an arm alone can't do well. With the eye beyond a pillar, the line
//! gets blocked all at once, and pulling in means crossing the pillar. So an occluder that's shallow
//! along the line, with the eye's own spot clear beyond it, is passed behind: the arm stays out, and
//! the player is hidden for the moment the line takes to pass. A second cast, back from the eye,
//! measures how deep it is. Depth alone can't tell a pillar from a long thin wall, so only for
//! `thin_patience` seconds, and the arm comes in front of whatever still hides the player after that.
//!
//! **When sliding can't get there,** as with the eye shut behind a thick wall, it counts as stuck: after
//! `stuck_time` the eye hops to its place along the line, which is free by that line's cast. Half way
//! there, with its place still out of a straight sweep's reach, it starts walking in by the player's
//! chest, which sees its place, so it mostly comes round the corner and has no need to. A hop jumps by
//! design, is counted in [`CameraStats::hops`], and is the one exception to the jerk bound; leading, sliding
//! and walking make it rare.
//!
//! **An eye pulled in within `CRAMPED` of the player's chest** sees neither fighter, so it comes out as
//! quickly as it went in, with no delay: there's nothing there to keep steady. What gets it room in the first
//! place is turning the view: the `clearing` module.
//!
//! **A takeover's eye** ([`TakeoverEye`]) has no arm: a shot's author says where it stands, and nothing need
//! see the player from there. The second half of the rule holds it all the same: it sweeps and slides from
//! where it was to where the shot or the blend wants it, a place in a wall meaning the nearest one out of it,
//! and hops there as the arm's eye does if sliding can't get it there. Nothing in the way, and it's exactly there.

use sapphire_math::Vec3;
use sapphire_physics::{Group, PhysicsWorld, Pose, QueryFilter, Shape};

use crate::director::CameraStats;
use crate::spring::Follow;
use crate::tuning::CollisionTuning;

/// How far the eye stays off a surface it stops at, in metres.
const SKIN: f32 = 0.002;
/// Sweeps a slide may take in a frame: enough for an inside corner.
const SLIDE_BOUNCES: u32 = 3;
/// Motion too small to sweep, in metres.
const NO_MOVE: f32 = 1e-5;
/// An eye nearer the player's chest than this, in metres, is in their face.
pub(crate) const CRAMPED: f32 = 1.5;
/// How far the line must move over the lead time before it's worth a second cast, in metres.
const WORTH_LEADING: f32 = 0.01;
/// Pushes to get a place out of everything it's in: enough for an inside corner.
const PUSHES_OUT: u32 = 4;

/// What's along a line, from its start to its end.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Clear,
    /// Shallow, with the line's end clear beyond it. `hit` is how far the line gets.
    Thin {
        hit: f32,
    },
    Blocked {
        hit: f32,
    },
    /// The sphere overlaps something where the line starts.
    StartsInside,
}

/// The eye's state from frame to frame.
#[derive(Clone, Debug)]
pub(crate) struct Arm {
    sphere: Shape,
    filter: QueryFilter,
    /// Metres the eye sits nearer the player than the rig asked for.
    pull: Follow<f32>,
    /// Where the eye was left: free of every collider.
    eye: Vec3,
    /// How far the eye sits from where the arm wants it. Nothing while it moves freely. A surface opens this
    /// up, and it closes on a spring when the eye can move again, rather than all at once.
    gap: Follow<Vec3>,
    last_proposed: Vec3,
    last_start: Vec3,
    last_goal: Vec3,
    /// Seconds since the line last asked for this much pull or more.
    since_pulled: f32,
    /// Seconds a thin occluder has been on the line.
    thin_for: f32,
    /// Seconds the eye has been away from where the arm wants it.
    stuck_for: f32,
}

impl Arm {
    pub(crate) fn new(radius: f32) -> Self {
        Self {
            sphere: Shape::Sphere { radius },
            filter: QueryFilter::new(Group::World.mask() | Group::CameraBlocker.mask()),
            pull: Follow::new(0.0),
            eye: Vec3::ZERO,
            gap: Follow::new(Vec3::ZERO),
            last_proposed: Vec3::ZERO,
            last_start: Vec3::ZERO,
            last_goal: Vec3::ZERO,
            since_pulled: 0.0,
            thin_for: 0.0,
            stuck_for: 0.0,
        }
    }

    /// A reload's new radius. The next frame's sweep starts from an overlap at worst, and leaves it.
    pub(crate) fn set_radius(&mut self, radius: f32) {
        self.sphere = Shape::Sphere { radius };
    }

    /// Where the eye goes this frame, given the rig's `goal` and the `anchor` it must keep in sight.
    /// With `snap` it goes straight there, or to the nearest place the line reaches.
    #[allow(clippy::too_many_arguments, reason = "one frame's inputs, at its one call")]
    pub(crate) fn resolve(
        &mut self,
        world: &PhysicsWorld,
        anchor: Vec3,
        goal: Vec3,
        tuning: &CollisionTuning,
        dt: f32,
        snap: bool,
        stats: &mut CameraStats,
    ) -> Vec3 {
        let thin = tuning.thin_occluder;
        let mut start = anchor;
        let mut line = classify(world, &self.sphere, &self.filter, start, goal, thin, stats);
        if line == Line::StartsInside {
            // A character up against a wall can have a chest closer to it than the sphere's radius.
            start = self.pushed_out(world, anchor, stats);
            line = classify(world, &self.sphere, &self.filter, start, goal, thin, stats);
        }
        let offset = goal - start;
        let length = offset.length();
        if length < NO_MOVE {
            self.eye = start;
            return start;
        }
        let direction = offset / length;
        let patient =
            self.thin_for < tuning.thin_patience && self.stuck_for < tuning.stuck_time * 0.5 && !snap;

        match line {
            Line::Clear | Line::StartsInside => self.thin_for = 0.0,
            Line::Thin { .. } => {
                self.thin_for += dt;
                stats.thin_frames += 1;
            }
            Line::Blocked { .. } => {}
        }
        let mut wanted = pull_for(line, length, patient);

        if snap {
            self.pull.reset(wanted);
            self.since_pulled = 0.0;
            self.thin_for = 0.0;
            self.stuck_for = 0.0;
            self.gap.reset(Vec3::ZERO);
            self.eye = start + direction * (length - wanted);
            self.last_proposed = self.eye;
        } else {
            if dt > 0.0 {
                let (start_soon, goal_soon) = (
                    start + (start - self.last_start) * (tuning.lead_time / dt),
                    goal + (goal - self.last_goal) * (tuning.lead_time / dt),
                );
                let worth = start_soon.distance_squared(start) > WORTH_LEADING * WORTH_LEADING
                    || goal_soon.distance_squared(goal) > WORTH_LEADING * WORTH_LEADING;
                if worth {
                    let soon =
                        classify(world, &self.sphere, &self.filter, start_soon, goal_soon, thin, stats);
                    // How far that line gets, as a pull on this one: only what's in the way counts, not
                    // that the line's about to be longer.
                    wanted = wanted.max(pull_for(soon, length, patient));
                }
            }
            self.pull_toward(wanted.min(length), length, tuning, dt);

            let proposed = start + direction * (length - self.pull.value);
            let mut aim = proposed + self.closed_gap(tuning, dt);
            // Slid as far as it goes, with its place round a corner: it comes round by the player's chest,
            // which sees its place, rather than wait to hop there.
            if self.stuck_for >= tuning.stuck_time * 0.5
                && self.reach(world, self.eye, aim, stats) + NO_MOVE < self.eye.distance(aim)
            {
                aim = self.eye + (start - self.eye).clamp_length_max(tuning.max_pull_speed * dt);
            }
            let eye = self.slide(world, self.eye, aim, stats);
            self.last_proposed = proposed;
            self.stuck_for = if eye.distance_squared(proposed) > tuning.stuck_gap * tuning.stuck_gap {
                self.stuck_for + dt
            } else {
                0.0
            };
            self.eye = eye;
            if self.stuck_for >= tuning.stuck_time && dt > 0.0 {
                // To the eye's place, short of the near side of whatever's on the line.
                let reach = match line {
                    Line::Thin { hit } | Line::Blocked { hit } => hit,
                    Line::Clear | Line::StartsInside => length,
                };
                let arm = (length - self.pull.value).min(reach);
                self.pull.reset(length - arm);
                self.eye = start + direction * arm;
                self.last_proposed = self.eye;
                self.gap.reset(Vec3::ZERO);
                self.stuck_for = 0.0;
                self.thin_for = self.thin_for.max(tuning.thin_patience);
                stats.hops += 1;
            }
        }

        if self.pull.value > 0.0 {
            stats.pulled_frames += 1;
        }
        self.last_start = start;
        self.last_goal = goal;
        self.eye
    }

    /// Where the eye stands, before anything moves the drawn pose off it.
    pub(crate) fn eye(&self) -> Vec3 {
        self.eye
    }

    /// How far along the line from `start` to `end` the eye's sphere can stand, in metres. A start in
    /// something counts from outside it, as [`Self::resolve`] takes it.
    pub(crate) fn reach(&self, world: &PhysicsWorld, start: Vec3, end: Vec3, stats: &mut CameraStats) -> f32 {
        let offset = end - start;
        let length = offset.length();
        if length < NO_MOVE {
            return 0.0;
        }
        stats.casts += 1;
        let direction = offset / length;
        let front = world.cast_shape(&self.sphere, Pose::at(start), direction, length, &self.filter);
        let Some(hit) = front else { return length };
        if !hit.started_overlapping {
            return (hit.distance - SKIN).max(0.0);
        }
        let clear = self.pushed_out(world, start, stats);
        stats.casts += 1;
        world
            .cast_shape(&self.sphere, Pose::at(clear), direction, length, &self.filter)
            .map_or(length, |hit| if hit.started_overlapping { 0.0 } else { (hit.distance - SKIN).max(0.0) })
    }

    /// Moves the eye by a shake's offset, stopping short of anything in the way. The result isn't
    /// kept: next frame's eye moves from where the arm left it.
    pub(crate) fn offset_by(
        &self,
        world: &PhysicsWorld,
        eye: Vec3,
        offset: Vec3,
        stats: &mut CameraStats,
    ) -> Vec3 {
        let length = offset.length();
        if length < NO_MOVE {
            return eye;
        }
        let direction = offset / length;
        stats.casts += 1;
        let reach = world
            .cast_shape(&self.sphere, Pose::at(eye), direction, length, &self.filter)
            .map_or(length, |hit| (hit.distance - SKIN).max(0.0));
        eye + direction * reach
    }

    /// What's left of the eye's gap from its place after this step.
    fn closed_gap(&mut self, tuning: &CollisionTuning, dt: f32) -> Vec3 {
        closed_gap(&mut self.gap, self.eye - self.last_proposed, tuning, dt)
    }

    /// Moves the pull toward `wanted`: quickly in, and slowly out once the line has stayed clear. An eye in
    /// the player's face goes out as soon as it can, as quickly as it came: it sees nothing from there, so
    /// there's nothing to keep steady.
    fn pull_toward(&mut self, wanted: f32, length: f32, tuning: &CollisionTuning, dt: f32) {
        let now = self.pull.value;
        let coming_in = wanted >= now - 1e-3;
        self.since_pulled = if coming_in { 0.0 } else { self.since_pulled + dt };
        let cramped = length - now < CRAMPED;
        let (goal, half_life) = if coming_in || cramped {
            (wanted, tuning.pull_in_half_life)
        } else if self.since_pulled >= tuning.pull_out_delay {
            (wanted, tuning.pull_out_half_life)
        } else {
            (now, tuning.pull_out_half_life)
        };
        self.pull.update(goal, half_life, dt);
        let limit = tuning.max_pull_speed * dt;
        self.pull.value = now + (self.pull.value - now).clamp(-limit, limit);
        self.pull.velocity = self.pull.velocity.clamp(-tuning.max_pull_speed, tuning.max_pull_speed);
        // A spring never quite arrives: below this, the arm is out.
        if self.pull.value <= NO_MOVE * 100.0 && goal <= 0.0 {
            self.pull.reset(0.0);
        }
    }

    /// Sweeps the eye from `from` to `to`, sliding along what it meets. Starting and ending free of
    /// every collider, whatever `to` is: this is what holds the no-clip promise.
    fn slide(&self, world: &PhysicsWorld, from: Vec3, to: Vec3, stats: &mut CameraStats) -> Vec3 {
        let mut at = from;
        let mut goal = to;
        for _ in 0..SLIDE_BOUNCES {
            let along = goal - at;
            let length = along.length();
            if length < NO_MOVE {
                break;
            }
            let direction = along / length;
            stats.casts += 1;
            let Some(hit) = world.cast_shape(&self.sphere, Pose::at(at), direction, length, &self.filter)
            else {
                at = goal;
                break;
            };
            stats.contacts += 1;
            if hit.started_overlapping {
                at += hit.normal * SKIN;
                at = self.pushed_out(world, at, stats);
                continue;
            }
            at += direction * hit.distance + hit.normal * SKIN;
            let rest = goal - at;
            goal = at + rest - hit.normal * rest.dot(hit.normal);
        }
        // A sweep lets a grazing path go, so a path grazing a surface can end a whisker into it.
        self.pushed_out(world, at, stats)
    }

    /// `at`, or out of what it overlaps by the shortest way.
    fn pushed_out(&self, world: &PhysicsWorld, at: Vec3, stats: &mut CameraStats) -> Vec3 {
        stats.casts += 1;
        match world.penetration(&self.sphere, Pose::at(at), &self.filter) {
            Some(deepest) => at + deepest.normal * (deepest.depth + SKIN),
            None => at,
        }
    }

    /// `at`, or the nearest place out of everything it overlaps, if a few pushes find one: walls can stand
    /// too close together for the eye's sphere to fit between them.
    fn clear_of_everything(&self, world: &PhysicsWorld, at: Vec3, stats: &mut CameraStats) -> Option<Vec3> {
        let mut at = at;
        for _ in 0..PUSHES_OUT {
            stats.casts += 1;
            match world.penetration(&self.sphere, Pose::at(at), &self.filter) {
                Some(deepest) => at += deepest.normal * (deepest.depth + SKIN),
                None => return Some(at),
            }
        }
        None
    }
}

/// The eye while a takeover has any of the view: see the [module docs](self). The arm goes on beneath it, so
/// the rigs have a current view to hand back to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TakeoverEye {
    eye: Vec3,
    /// How far the eye sits from its place: as the arm's, nothing while it moves freely.
    gap: Follow<Vec3>,
    /// Its place last frame.
    last_place: Vec3,
    /// Seconds it's been away from its place.
    stuck_for: f32,
}

impl TakeoverEye {
    /// Setting out from `eye`, which is clear: the arm's, as the takeover gets its first share of the view.
    pub(crate) fn at(eye: Vec3) -> Self {
        Self { eye, gap: Follow::new(Vec3::ZERO), last_place: eye, stuck_for: 0.0 }
    }

    /// Whether the eye's on its place, which it needn't have been for a shot to be over.
    pub(crate) fn is_there(&self) -> bool {
        self.eye == self.last_place
    }

    /// Where the eye goes this frame, for a shot or a blend that wants it at `goal`. With `snap` it goes
    /// straight there, or to the nearest place it fits, however far that is from where it was. The eye's
    /// sphere ends clear of every collider, and with nothing in the way the eye is exactly `goal`.
    #[allow(clippy::too_many_arguments, reason = "one frame's inputs, at its one call")]
    pub(crate) fn step(
        &mut self,
        arm: &Arm,
        world: &PhysicsWorld,
        goal: Vec3,
        tuning: &CollisionTuning,
        dt: f32,
        snap: bool,
        stats: &mut CameraStats,
    ) -> Vec3 {
        let clear = arm.clear_of_everything(world, goal, stats);
        if snap {
            // With nowhere near there to stand, as far along the way from what's known to be clear.
            self.eye = clear.unwrap_or_else(|| arm.slide(world, arm.eye(), goal, stats));
            self.gap.reset(Vec3::ZERO);
            self.last_place = self.eye;
            self.stuck_for = 0.0;
            return self.eye;
        }
        let place = clear.unwrap_or(goal);
        let aim = place + closed_gap(&mut self.gap, self.eye - self.last_place, tuning, dt);
        self.eye = arm.slide(world, self.eye, aim, stats);
        self.last_place = place;
        self.stuck_for = if self.eye.distance_squared(place) > tuning.stuck_gap * tuning.stuck_gap {
            self.stuck_for + dt
        } else {
            0.0
        };
        if self.stuck_for >= tuning.stuck_time
            && let Some(clear) = clear
        {
            self.eye = clear;
            self.gap.reset(Vec3::ZERO);
            self.stuck_for = 0.0;
            stats.hops += 1;
        }
        self.eye
    }
}

/// What's left of `gap`, from an eye to its place, after this step, closing no faster than the arm moves.
fn closed_gap(spring: &mut Follow<Vec3>, gap: Vec3, tuning: &CollisionTuning, dt: f32) -> Vec3 {
    if gap.length_squared() < NO_MOVE * NO_MOVE {
        spring.reset(Vec3::ZERO);
        return Vec3::ZERO;
    }
    spring.value = gap;
    let next = spring.update(Vec3::ZERO, tuning.pull_in_half_life, dt);
    let limit = tuning.max_pull_speed * dt;
    let closed = (gap - next).clamp_length_max(limit);
    spring.value = gap - closed;
    spring.velocity = spring.velocity.clamp_length_max(tuning.max_pull_speed);
    spring.value
}

/// How much pull a line asks for. A thin occluder asks for none while there's patience left.
fn pull_for(line: Line, length: f32, patient: bool) -> f32 {
    match line {
        Line::Clear | Line::StartsInside => 0.0,
        Line::Thin { .. } if patient => 0.0,
        Line::Thin { hit } | Line::Blocked { hit } => (length - hit).max(0.0),
    }
}

/// What's on the line from `start` to `end`, for the sphere.
fn classify(
    world: &PhysicsWorld,
    sphere: &Shape,
    filter: &QueryFilter,
    start: Vec3,
    end: Vec3,
    thin_occluder: f32,
    stats: &mut CameraStats,
) -> Line {
    let offset = end - start;
    let length = offset.length();
    if length < NO_MOVE {
        return Line::Clear;
    }
    let direction = offset / length;
    stats.casts += 1;
    let Some(front) = world.cast_shape(sphere, Pose::at(start), direction, length, filter) else {
        return Line::Clear;
    };
    if front.started_overlapping {
        return Line::StartsInside;
    }
    let hit = (front.distance - SKIN).max(0.0);
    stats.casts += 1;
    let reach = length - front.distance;
    match world.cast_shape(sphere, Pose::at(end), -direction, reach, filter) {
        Some(back) if back.started_overlapping => Line::Blocked { hit },
        Some(back) if reach - back.distance > thin_occluder => Line::Blocked { hit },
        _ => Line::Thin { hit },
    }
}
