//! Turning the view round what's in its way.
//!
//! The arm ([`crate::collision`]) answers one question: how far back can the eye stand along this line?
//! With a stall behind the player the answer is "nowhere", and the eye collapses onto the player's chest,
//! looking at a point past them, so neither fighter is in the frame. A person's thumb would turn the view
//! until there was room. This does the same, slowly, and only when it's worth it:
//!
//! - **It looks at the view it has,** every `check_every` seconds. [`Scene::fault`] puts a number on what a
//!   person would call wrong with a view from that yaw: a short arm, a fighter out of the frame or behind
//!   something, the target in front of the player or filling the frame, or something standing nearer than
//!   either of them.
//! - **A view that's bad for `patience` seconds gets compared** with others either side, out to `reach`.
//!   The best that's better by `better_by`, counting what the turn costs, becomes the goal, and the yaw
//!   turns there at `half_life`, through the rigs' own pull, so it starts and stops as softly as any rig. Then
//!   nothing new is chosen for `settle` seconds: a slow steady camera beats a twitchy correct one.
//! - **It never fights a thumb:** look input drops the goal, and for `after_look` seconds nothing's chosen.
//!   Under lock-on, which holds the line, what it chooses is a lean off the line instead, which comes back
//!   when the line's clear again. Up close, where `line_fade` has lock-on let go of the line, it's free.
//!
//! A view with nothing wrong with it costs about a dozen casts ten times a second, and one being compared
//! thirteen times that, and open ground never turns unless the enemy stands over the player. It reads nothing
//! but the frame and the world, so it can't move a fight: the simulation never reads the camera. `heading()`
//! doesn't carry a shake through it either: it reads the arm's own eye, never the drawn one.

use sapphire_math::det::wrap_angle;
use sapphire_math::{UP, Vec3};
use sapphire_physics::{Group, PhysicsWorld, QueryFilter};

use crate::collision::Arm;
use crate::director::{CameraStats, looking, shut_in};
use crate::rigs::{Axes, Subject, framed_distance};
use crate::spring::Follow;
use crate::tuning::{ClearingTuning, FramingTuning};

/// The yaws compared with the view's own, as shares of `reach`, each way.
const SHARES: [f32; 6] = [0.12, 0.25, 0.4, 0.55, 0.75, 1.0];
/// A short arm, as a share of what the rig asks for: it costs this much at none of it left.
const SHORT_ARM: f32 = 0.75;
/// Standing closer to the player's chest than this, in metres, costs up to [`TOO_CLOSE`] on top.
const ROOM: f32 = 2.5;
const TOO_CLOSE: f32 = 1.5;
/// Past this much of the screen's half-size a fighter counts as leaving it, and it costs up to [`OUT`] by
/// [`OUT_RANGE`] past that.
const EDGE: f32 = 0.9;
const OUT_RANGE: f32 = 0.5;
const OUT: f32 = 1.5;
/// Each fighter's head or chest behind something.
const HIDDEN: f32 = 0.5;
/// The target over the player: nothing up to [`COVER_FREE`] of them, and this much at all of them.
const COVER: f32 = 1.0;
const COVER_FREE: f32 = 0.2;
/// The target taller than this share of the frame, and what it costs at all of it.
const FILLS_FROM: f32 = 0.7;
const FILLS: f32 = 1.0;
/// What turning costs a radian: of views about as good, the nearest.
const TURNING: f32 = 0.2;
/// How much a bad view speeds the turn away from it, for each unit of what's wrong with it.
const HASTE: f32 = 1.5;
/// How near its goal counts as there, in radians.
const THERE: f32 = 0.03;
/// Drawing nearer its goal slower than this, in radians a second, is as near as it's getting: a rig's own pull
/// is holding it short, and standing by that goal would be waiting there for good.
const STALLED: f32 = 0.02;
/// How long, in seconds, it doesn't choose again the way the rigs held it short. Long enough that turning back
/// and forth doesn't become a habit, and short enough that a fight that's moved on isn't kept from that side.
const HELD_FOR: f32 = 5.0;
/// How near a check's moment counts as it, in seconds: frames only add up to it to within rounding.
const ON_TIME: f32 = 1e-4;
/// How wide someone is taken to draw, on top of their radius, in metres. A fighter's arms and weapon reach
/// past the capsule.
const DRAWN_WIDER: f32 = 0.1;
/// The target nearer the camera than the player: what a view asks to avoid even with nothing covered, up
/// to this at [`IN_FRONT_RANGE`] metres nearer.
const IN_FRONT: f32 = 0.6;
const IN_FRONT_RANGE: f32 = 3.0;
/// A view the eye can't get to in a straight line from where it is: it would stop on what's between.
const NO_WAY: f32 = 0.4;
/// What stands nearer than the nearer fighter, on these screen points, costs up to this.
const FOREGROUND: f32 = 0.6;
const FOREGROUND_POINTS: [(f32, f32); 5] = [(0.0, 0.0), (-0.5, 0.3), (0.5, 0.3), (-0.5, -0.3), (0.5, -0.3)];
/// What stands lower than this over the ground doesn't count as in front: it's what the view looks down on.
const LOW: f32 = 0.45;

/// What's being framed, for judging a view.
pub(crate) struct Scene<'a> {
    pub world: &'a PhysicsWorld,
    pub arm: &'a Arm,
    /// Where the arm starts: what it keeps in sight.
    pub anchor: Vec3,
    pub player: &'a Subject,
    pub target: &'a Subject,
    /// What the view looks at.
    pub pivot: Vec3,
    pub pitch: f32,
    pub aspect: f32,
    /// The tangent of half the vertical field of view.
    pub half_fov: f32,
    pub framing: &'a FramingTuning,
    /// The factor zoom puts on the distance.
    pub zoom: f32,
    /// Where the fighters will be in a moment, for what's about to go wrong.
    pub player_soon: Vec3,
    pub target_soon: Vec3,
}

/// Where a subject lands on the screen.
struct Drawn {
    left: f32,
    right: f32,
    feet: f32,
    head: f32,
    depth: f32,
}

/// What's wrong with a view, part by part.
#[derive(Clone, Copy, Debug, Default)]
struct Faults {
    short_arm: f32,
    too_close: f32,
    out: f32,
    fills: f32,
    covered: f32,
    in_front: f32,
    foreground: f32,
    hidden: f32,
    no_way: f32,
}

impl Faults {
    /// 0 for nothing, and about 1 for something a person would turn away from.
    fn total(&self) -> f32 {
        self.short_arm
            + self.too_close
            + self.out
            + self.fills
            + self.covered
            + self.in_front
            + self.foreground
            + self.hidden
            + self.no_way
    }

    /// What's enough to go looking for another view. The target merely being the nearer isn't: it's what
    /// a choice between views should weigh, but with a fight going round the player, turning for it alone
    /// would have the view chasing them.
    fn troubling(&self) -> f32 {
        self.total() - self.in_front
    }
}

impl Scene<'_> {
    /// How much is wrong with a view from `yaw`: 0 for nothing, about 1 for something a person would turn
    /// away from.
    fn fault(&self, yaw: f32, stats: &mut CameraStats) -> f32 {
        self.faults(yaw, stats).total()
    }

    /// What's wrong with a view from `yaw`. See the constants for how each part counts.
    fn faults(&self, yaw: f32, stats: &mut CameraStats) -> Faults {
        let axes = Axes::of(yaw, self.pitch);
        let framing = self.framing;
        let (tan_x, tan_y) =
            (self.half_fov * self.aspect * framing.safe_frame[0], self.half_fov * framing.safe_frame[1]);
        let distance =
            framed_distance([self.player, self.target], self.pivot, &axes, tan_x, tan_y, framing.min_depth)
                .max(framing.distance * self.zoom)
                .min(framing.max_distance);
        let goal = self.pivot - axes.forward * distance;
        let mut faults = Faults::default();

        // How far the arm reaches.
        let arm = goal - self.anchor;
        let length = arm.length();
        let reach = self.arm.reach(self.world, self.anchor, goal, stats);
        if length > 1e-3 {
            faults.short_arm = SHORT_ARM * (1.0 - reach / length).max(0.0);
        }
        faults.too_close = TOO_CLOSE * ((ROOM.min(length) - reach) / ROOM).max(0.0);
        let eye = self.anchor + arm.normalize_or_zero() * reach;

        // Who's in the frame from there, looking where the director would.
        let framed = self.target.position + UP * self.target.height * 0.55;
        let share = shut_in(eye, self.anchor, framed);
        let forward = looking(eye, self.pivot, axes.forward, Some(framed), share)
            .try_normalize()
            .unwrap_or(axes.forward);
        let right = forward.cross(UP).try_normalize().unwrap_or(axes.right);
        let up = right.cross(forward);
        let on_screen = |point: Vec3| {
            let offset = point - eye;
            let depth = offset.dot(forward);
            (offset.dot(right), offset.dot(up), depth)
        };
        let half = self.half_fov;
        let aspect = self.aspect;
        let drawn = |subject: &Subject| -> Option<Drawn> {
            let (x0, y0, d0) = on_screen(subject.position);
            let (x1, y1, d1) = on_screen(subject.position + UP * subject.height);
            if d0 <= 0.05 || d1 <= 0.05 {
                return None;
            }
            let (x0, y0) = (x0 / (d0 * half * aspect), y0 / (d0 * half));
            let (x1, y1) = (x1 / (d1 * half * aspect), y1 / (d1 * half));
            let wide = (subject.radius + DRAWN_WIDER) / (d0.min(d1) * half * aspect);
            Some(Drawn {
                left: x0.min(x1) - wide,
                right: x0.max(x1) + wide,
                feet: y0.min(y1),
                head: y0.max(y1),
                depth: d0.min(d1),
            })
        };
        let (player, target) = (drawn(self.player), drawn(self.target));
        for subject in [&player, &target] {
            faults.out += match subject {
                None => OUT,
                Some(subject) => {
                    let edge = subject
                        .left
                        .abs()
                        .max(subject.right.abs())
                        .max(subject.head.abs())
                        .max(subject.feet.abs());
                    OUT * ((edge - EDGE) / OUT_RANGE).clamp(0.0, 1.0)
                }
            };
        }
        if let Some(target) = &target {
            let tall = (target.head - target.feet) * 0.5;
            faults.fills = FILLS * ((tall - FILLS_FROM) / (1.0 - FILLS_FROM)).clamp(0.0, 1.0);
        }
        let soon = (
            drawn(&Subject { position: self.player_soon, ..*self.player }),
            drawn(&Subject { position: self.target_soon, ..*self.target }),
        );
        let covered = covered_share(player.as_ref(), target.as_ref())
            .max(covered_share(soon.0.as_ref(), soon.1.as_ref()));
        faults.covered = COVER * ((covered - COVER_FREE) / (1.0 - COVER_FREE)).max(0.0);
        if let (Some(player), Some(target)) = (&player, &target) {
            faults.in_front = IN_FRONT * ((player.depth - target.depth) / IN_FRONT_RANGE).clamp(0.0, 1.0);
        }

        // What's standing between the eye and them: a stall's side across half the frame, say.
        let nearest = match (&player, &target) {
            (Some(player), Some(target)) => player.depth.min(target.depth),
            (Some(subject), None) | (None, Some(subject)) => subject.depth,
            (None, None) => 0.0,
        };

        // Who's behind something: the player's chest is where the arm starts, so only their head.
        let sees = QueryFilter::new(Group::World.mask() | Group::CameraBlocker.mask());
        if nearest > 0.5 {
            let mut blocked = 0_u32;
            for (x, y) in FOREGROUND_POINTS {
                let along = forward + right * (x * half * aspect) + up * (y * half);
                stats.casts += 1;
                let far = nearest * along.length();
                if self.world.cast_ray(eye, along, far, &sees).is_some_and(|hit| hit.point.y > LOW) {
                    blocked += 1;
                }
            }
            faults.foreground = FOREGROUND * blocked as f32 / FOREGROUND_POINTS.len() as f32;
        }
        let mut hidden = |point: Vec3| {
            let along = point - eye;
            let far = along.length() - 0.1;
            if far <= 0.0 {
                return false;
            }
            stats.casts += 1;
            self.world.cast_ray(eye, along, far, &sees).is_some()
        };
        let (target_chest, target_head) = (
            self.target.position + UP * (self.target.height * 0.55),
            self.target.position + UP * self.target.height,
        );
        faults.hidden += HIDDEN * f32::from(u8::from(hidden(target_chest)));
        faults.hidden += HIDDEN * f32::from(u8::from(hidden(target_head)));
        faults.hidden += HIDDEN * f32::from(u8::from(hidden(self.player.position + UP * self.player.height)));
        // From the arm's own eye, which no shake moves: what the stick's heading follows can't carry one.
        let now = self.arm.eye();
        if self.arm.reach(self.world, now, eye, stats) + 0.1 < now.distance(eye) {
            faults.no_way = NO_WAY;
        }
        faults
    }
}

/// How much of `player` the target covers, with the target the nearer.
fn covered_share(player: Option<&Drawn>, target: Option<&Drawn>) -> f32 {
    let (Some(player), Some(target)) = (player, target) else { return 0.0 };
    if target.depth >= player.depth {
        return 0.0;
    }
    let across = (player.right.min(target.right) - player.left.max(target.left)).max(0.0);
    let tall = (player.head.min(target.head) - player.feet.max(target.feet)).max(0.0);
    let whole = (player.right - player.left) * (player.head - player.feet);
    across * tall / whole.max(1e-6)
}

/// What the view's turning toward, and when it may choose again.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Clearing {
    /// Since the last check, in seconds.
    since_check: f32,
    /// How long the view's been bad.
    troubled: f32,
    /// How long before it may choose again.
    until_free: f32,
    /// Where a free view is turning: its yaw.
    goal: Option<f32>,
    /// How far off that goal was at the last check.
    goal_left: f32,
    /// Which way the rigs last held a goal short, and for how long that still counts.
    held_side: f32,
    held_for: f32,
    /// How much faster than `half_life` it's turning: the worse the view it left, the less there is to
    /// keep steady.
    haste: f32,
    /// How far off the line a held view is leaning, and how far it's asked to.
    lean_goal: f32,
    lean: Follow<f32>,
}

impl Clearing {
    pub(crate) fn new() -> Self {
        Self {
            since_check: 0.0,
            troubled: 0.0,
            until_free: 0.0,
            goal: None,
            goal_left: 0.0,
            held_side: 0.0,
            held_for: 0.0,
            haste: 1.0,
            lean_goal: 0.0,
            lean: Follow::new(0.0),
        }
    }

    /// Everything off: a cut, or nobody to frame.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// A thumb's on the view: it's theirs.
    pub(crate) fn hand_over(&mut self, tuning: &ClearingTuning) {
        self.goal = None;
        self.held_for = 0.0;
        self.troubled = 0.0;
        self.until_free = self.until_free.max(tuning.after_look);
    }

    /// The yaw a free view is turning to, if any.
    pub(crate) fn goal(&self) -> Option<f32> {
        self.goal
    }

    /// How much faster than `half_life` the view turns.
    pub(crate) fn haste(&self) -> f32 {
        self.haste
    }

    /// How far off its line a held view leans.
    pub(crate) fn lean(&self) -> f32 {
        self.lean.value
    }

    /// Runs a step. `yaw` is where the view's heading, and `held` says it holds a line, so that's what
    /// a choice leans off.
    pub(crate) fn step(
        &mut self,
        scene: &Scene<'_>,
        tuning: &ClearingTuning,
        yaw: f32,
        held: bool,
        dt: f32,
        stats: &mut CameraStats,
    ) {
        if tuning.reach <= 0.0 {
            // All of it off, what it already chose included: a lean comes back as softly as it went.
            *self = Self { lean: self.lean, ..Self::new() };
            self.lean.update(0.0, tuning.half_life, dt);
            return;
        }
        self.lean.update(self.lean_goal, tuning.half_life, dt);
        if held {
            self.goal = None;
        }
        self.since_check += dt;
        if self.since_check < tuning.check_every - ON_TIME {
            return;
        }
        // Whole periods, and everything else that counts time counts them, so the same choices fall at the
        // same moments at any frame rate.
        let mut periods = 0_u16;
        while self.since_check >= tuning.check_every - ON_TIME {
            self.since_check -= tuning.check_every;
            periods += 1;
        }
        let elapsed = f32::from(periods) * tuning.check_every;
        self.until_free = (self.until_free - elapsed).max(0.0);
        self.held_for = (self.held_for - elapsed).max(0.0);
        // A goal the rigs are holding the view short of.
        let mut held_short = false;
        if let Some(goal) = self.goal {
            let off = wrap_angle(goal - yaw);
            if off.abs() < THERE {
                self.goal = None;
            } else if off.abs() > self.goal_left - STALLED * elapsed {
                held_short = true;
                self.held_side = off.signum();
                self.held_for = HELD_FOR;
            }
            self.goal_left = off.abs();
        }

        let faults = scene.faults(yaw + if held { self.lean_goal } else { 0.0 }, stats);
        let here = faults.total();
        if faults.troubling() <= tuning.tolerance {
            self.troubled = 0.0;
            // Nothing's wrong with as far as it's got, so that's far enough.
            if held_short {
                self.goal = None;
            }
            // The line's clear again: back to it, once it's clear from the line itself.
            if held && self.lean_goal != 0.0 && scene.faults(yaw, stats).troubling() <= tuning.tolerance {
                self.lean_goal = 0.0;
            }
            return;
        }
        self.troubled += elapsed;
        if self.troubled < tuning.patience || self.until_free > 0.0 {
            return;
        }

        // Past a goal already on its way, it stands by that one unless it's gone bad too.
        if !held
            && !held_short
            && let Some(goal) = self.goal
            && scene.fault(goal, stats) <= here
        {
            return;
        }
        let from = yaw + if held { self.lean_goal } else { 0.0 };
        let mut best = (here, 0.0);
        for share in SHARES {
            for side in [-1.0, 1.0] {
                // What held it short that way would again. If the other way has nothing better it stays
                // held where it is, which is at least steady.
                if !held && self.held_for > 0.0 && side == self.held_side {
                    continue;
                }
                let turn = share * tuning.reach * side;
                // A lean goes no further than `reach`, so there's nothing to choose past it.
                if held && (self.lean_goal + turn).abs() > tuning.reach {
                    continue;
                }
                let fault = scene.fault(from + turn, stats) + TURNING * turn.abs();
                if fault < best.0 {
                    best = (fault, turn);
                }
            }
        }
        if best.1 != 0.0 && best.0 + tuning.better_by < here {
            stats.swings += 1;
            self.haste = 1.0 + HASTE * here.min(1.5);
            self.until_free = tuning.settle;
            self.troubled = 0.0;
            if held {
                self.lean_goal += best.1;
                self.lean_goal = self.lean_goal.clamp(-tuning.reach, tuning.reach);
            } else {
                self.goal = Some(from + best.1);
                self.goal_left = best.1.abs();
            }
        }
    }
}
