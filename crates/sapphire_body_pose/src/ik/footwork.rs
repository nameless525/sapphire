//! The heel, ball and shoe orientation of a start or braking step are authored together.
//! Distances are measured from the solved support, with the gameplay root free to keep moving.

use sapphire_math::ease::smoothstep;
use sapphire_math::{Affine3A, Quat, Vec3};

use super::feet::FootContact;
use super::gather::SoleHistory;
use crate::graph::ground_footwork::{BRAKE_SETTLED, LAUNCH_CATCH};
use crate::pose_math::PoseMath;

const HEEL: usize = 0;
const BALL: usize = 1;
const GAIT_JOIN_TICKS: f32 = 3.0;

type History<'a, F> = crate::graph::canonical::Canonical<'a, F>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ShoeTurns {
    pub shoe: Quat,
    pub toes: Quat,
}

impl ShoeTurns {
    fn blend(self, other: Self, amount: f32, math: PoseMath) -> Self {
        Self {
            shoe: math.slerp(self.shoe, other.shoe, amount),
            toes: math.slerp(self.toes, other.toes, amount),
        }
    }

    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.floats(&self.shoe.to_array());
        out.floats(&self.toes.to_array());
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct DisplayedShoe {
    pub sole: SoleHistory,
    pub turns: ShoeTurns,
}

impl DisplayedShoe {
    fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        self.sole.write_canonical(out);
        self.turns.write_canonical(out);
    }
}

/// A complete shoe target, including the anatomical point that follows its path. Clearance is
/// measured at the lowest end of the rotated sole, so a late ankle turn cannot bury the tip.
#[derive(Clone, Copy)]
pub(super) struct ShoeTarget {
    pub pivot: usize,
    pub point: Vec3,
    pub clearance: f32,
    pub turns: ShoeTurns,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct FollowingGait {
    started: f32,
    from: DisplayedShoe,
    collection: f32,
}

impl FollowingGait {
    fn duration(self) -> f32 {
        GAIT_JOIN_TICKS + 3.0 * self.collection
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Brake {
        catch: f32,
        transfer: f32,
        vigor: f32,
        late: f32,
        already_arriving: bool,
        close_from: Option<DisplayedShoe>,
    },
    Launch {
        speed: f32,
        gait: f32,
        toe_off: f32,
        front_ball: Option<Vec3>,
        following: [Option<FollowingGait>; 2],
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Footwork {
    support: usize,
    initial: [DisplayedShoe; 2],
    facing: Quat,
    scale: f32,
    catch_heel: Vec3,
    rear_ball: Vec3,
    elapsed: f32,
    action: Action,
}

impl Footwork {
    pub(super) fn support_side(shoes: [Option<DisplayedShoe>; 2]) -> Option<usize> {
        match shoes {
            [Some(left), Some(right)] => {
                let grounded = |shoe: DisplayedShoe| shoe.sole.clearance().abs() < 0.014;
                if grounded(left) != grounded(right) {
                    return Some(usize::from(grounded(right)));
                }
                let facing =
                    (left.sole.ball() - left.sole.point(HEEL)) + (right.sole.ball() - right.sole.point(HEEL));
                let forward = facing.with_y(0.0).normalize_or(Vec3::Z);
                let ahead = (left.sole.ball() - right.sole.ball()).dot(forward).clamp(-0.7, 0.7);
                let cost = |shoe: DisplayedShoe| shoe.sole.support_cost();
                Some(usize::from(cost(right) + 0.16 * ahead < cost(left)))
            }
            _ => None,
        }
    }

    pub(super) fn matches(self, contact: FootContact) -> bool {
        match (self.action, contact) {
            (Action::Brake { .. }, FootContact::Brake { elapsed, support, .. })
            | (Action::Launch { .. }, FootContact::Launch { elapsed, support, .. }) => {
                elapsed >= self.elapsed && self.support == support
            }
            _ => false,
        }
    }

    pub(super) fn begin(
        contact: FootContact,
        shoes: [Option<DisplayedShoe>; 2],
        frame: &Affine3A,
        facing: Quat,
        scale: f32,
        leg_span: f32,
    ) -> Option<Self> {
        let initial = [shoes[0]?, shoes[1]?];
        let (FootContact::Brake { support, .. } | FootContact::Launch { support, .. }) = contact else {
            return None;
        };
        if support > 1 {
            return None;
        }
        let lead = 1 - support;
        let (support, remaining, elapsed, action) = match contact {
            FootContact::Brake { elapsed, support, remaining, speed } => {
                let vigor = smoothstep(1.0, 5.8, speed);
                let from_world = frame.inverse();
                let retained = initial[support].sole;
                let anchor = from_world.transform_point3(retained.point(retained.low_pivot()));
                let incoming = initial[lead].sole;
                let heel = from_world.transform_point3(incoming.point(HEEL));
                let trail = (-anchor.z).max(0.0);
                let late = smoothstep(leg_span * 0.17, leg_span * 0.40, trail);
                // A rolled shoe can stay behind the hip only until the root has used up the
                // remaining horizontal reach. Schedule the catch inside that travel budget.
                let room = (leg_span * 0.72 - trail).max(0.0);
                let ordinary = 6.0 + 1.2 * vigor;
                let budget =
                    if remaining > room { elapsed + 60.0 * room / speed.max(1.0) } else { ordinary + 2.0 };
                let catch = (budget - 1.2).clamp(3.35, ordinary);
                let transfer = catch + 1.1 + 0.9 * (1.0 - late);
                let already_arriving = incoming.clearance() > 0.055 * scale && heel.z > anchor.z + 0.10;
                (
                    support,
                    remaining,
                    elapsed,
                    Action::Brake { catch, transfer, vigor, late, already_arriving, close_from: None },
                )
            }
            FootContact::Launch { support, .. } => (
                support,
                0.0,
                0.0,
                Action::Launch {
                    speed: 0.0,
                    gait: 0.0,
                    toe_off: 0.0,
                    front_ball: None,
                    following: [None; 2],
                },
            ),
            _ => return None,
        };
        let vigor = match action {
            Action::Brake { vigor, .. } => vigor,
            Action::Launch { .. } => 0.0,
        };
        let catch_heel =
            frame.transform_point3(Vec3::new(track(lead), 0.0, remaining + 0.035 + 0.075 * vigor));
        let rear_ball = frame.transform_point3(Vec3::new(track(support), 0.0, remaining - 0.145));
        Some(Self { support, initial, facing, scale, catch_heel, rear_ball, elapsed, action })
    }

    pub(super) fn advance(
        &mut self,
        contact: FootContact,
        displayed: [Option<DisplayedShoe>; 2],
        frame: &Affine3A,
    ) {
        match (&mut self.action, contact) {
            (Action::Brake { transfer, close_from, .. }, FootContact::Brake { elapsed, .. }) => {
                if elapsed >= *transfer && close_from.is_none() {
                    *close_from = displayed[self.support];
                }
                self.elapsed = elapsed;
            }
            (
                Action::Launch { speed, gait, toe_off, front_ball, .. },
                FootContact::Launch { elapsed, speed: actual, gait: phase, toe_off: release, .. },
            ) => {
                if self.elapsed < 4.0 {
                    let remaining = (LAUNCH_CATCH - elapsed).max(0.0) / 60.0;
                    let acceleration =
                        ((actual - *speed) * 60.0 / (elapsed - self.elapsed).max(0.5)).clamp(0.0, 60.0);
                    let accelerating = ((6.0 - actual) / acceleration.max(1e-3)).clamp(0.0, remaining);
                    let end_speed = (actual + acceleration * accelerating).min(6.0);
                    let travel =
                        (actual + end_speed) * 0.5 * accelerating + end_speed * (remaining - accelerating);
                    let heel_lead = 0.191 + 0.043 * smoothstep(4.0, 2.1, end_speed);
                    let ahead = travel + heel_lead * (0.70 + 0.30 * smoothstep(0.5, 2.0, end_speed));
                    self.catch_heel = frame.transform_point3(Vec3::new(track(1 - self.support), 0.0, ahead));
                }
                if elapsed >= LAUNCH_CATCH + 3.0 && front_ball.is_none() {
                    *front_ball = displayed[1 - self.support].map(|shoe| shoe.sole.ball());
                }
                *speed = actual;
                *gait = phase;
                *toe_off = release;
                self.elapsed = elapsed;
            }
            _ => {}
        }
    }

    pub(super) fn settled(self) -> bool {
        matches!(self.action, Action::Brake { .. }) && self.elapsed >= BRAKE_SETTLED
    }

    pub(super) fn controls(self, side: usize) -> bool {
        let Action::Launch { following, .. } = self.action else { return true };
        following[side].is_none_or(|gait| self.elapsed < gait.started + gait.duration())
    }

    pub(super) fn receiving_shift(
        self,
        hips: Vec3,
        frame: &Affine3A,
        solved: [Option<SoleHistory>; 2],
    ) -> Vec3 {
        let Action::Brake { transfer, late, close_from: Some(_), .. } = self.action else {
            return Vec3::ZERO;
        };
        let Some(heel) = solved[1 - self.support].and_then(|sole| sole.grounded_point(HEEL, 0.014)) else {
            return Vec3::ZERO;
        };
        // The front heel already bears weight. Let the hips travel toward it after the rear shoe
        // has lifted, then finish the authored move into guard.
        let ahead = frame.inverse().transform_point3(heel).z - hips.z;
        let room = (ahead - 0.14).clamp(0.0, 0.066);
        let over_heel = smoothstep(transfer + 1.4, transfer + 3.3, self.elapsed);
        let settle = smoothstep(17.0, 12.0, self.elapsed);
        Vec3::Z * (room * late * over_heel * settle)
    }

    pub(super) fn goal(
        &mut self,
        side: usize,
        raw: DisplayedShoe,
        displayed: Option<DisplayedShoe>,
        math: PoseMath,
        advance: bool,
    ) -> ShoeTarget {
        let t = self.elapsed;
        let initial = self.initial[side];
        let scale = self.scale;
        let facing = self.facing;
        let shape = |pitch, toes| shoe_turns(facing, side, pitch, toes, math);
        let loaded = side == self.support;
        match &mut self.action {
            Action::Brake { catch, transfer, vigor, late, already_arriving, close_from } => {
                if !loaded {
                    let incoming = Swing {
                        target: self.catch_heel,
                        pivot: HEEL,
                        fold: shape(27.0, 18.0),
                        contact: shape(-14.0, -14.0),
                        peak: (0.105 + 0.055 * *vigor) * scale,
                        rise_by: 0.47,
                        travel: [0.0, 1.0],
                    };
                    if t < *catch {
                        let phase = t / *catch;
                        if *already_arriving {
                            incoming.descend(initial, phase, math)
                        } else {
                            incoming.sample(initial, phase, math)
                        }
                    } else {
                        ShoeTarget {
                            pivot: HEEL,
                            point: self.catch_heel,
                            clearance: 0.0,
                            turns: incoming.contact.blend(
                                shape(0.0, 0.0),
                                smoothstep(*catch, *catch + 3.5, t),
                                math,
                            ),
                        }
                    }
                } else if let Some(from) = *close_from {
                    let distance = (self.rear_ball - from.sole.ball()).with_y(0.0).length() / scale;
                    if distance < 0.075 {
                        return hold(from, BALL, shape(0.0, 0.0), 0.0);
                    }
                    let closing = Swing {
                        target: self.rear_ball,
                        pivot: BALL,
                        fold: shape(23.0 + 25.0 * *late, 15.0 + 8.0 * *late),
                        contact: shape(0.0, 0.0),
                        peak: (0.06 + distance * 0.07).min(0.105) * scale,
                        rise_by: 0.29,
                        // A long step lifts first, then tucks in sooner once it can clear.
                        travel: [0.07 * *late, 1.0 - 0.21 * *late],
                    };
                    let duration = (BRAKE_SETTLED - *transfer).min(10.0 - 2.5 * *late);
                    let phase = ((t - *transfer) / duration).clamp(0.0, 1.0);
                    closing.sample(from, phase, math)
                } else {
                    let pivot = initial.sole.low_pivot();
                    let landing = smoothstep(0.0, 2.5, t);
                    let roll = if pivot == BALL { 64.0 * *late } else { 0.0 };
                    let turns = initial.turns.blend(shape(roll, 0.0), smoothstep(0.0, 3.0, t), math);
                    hold(initial, pivot, turns, initial.sole.clearance().max(0.0) * (1.0 - landing))
                }
            }
            Action::Launch { speed, gait, toe_off, front_ball, following } => {
                let vigor = smoothstep(1.8, 5.4, *speed);
                let push = initial.turns.blend(
                    shape(30.0 + 24.0 * vigor, 0.0),
                    // Let the captured shoe take the initial load before rolling over its ball.
                    smoothstep(1.5, LAUNCH_CATCH + 1.0, t),
                    math,
                );
                let receiver_pitch = shape(-10.0, -10.0);
                let hold = if loaded {
                    hold(initial, BALL, push, 0.0)
                } else if t < LAUNCH_CATCH {
                    return Swing {
                        target: self.catch_heel,
                        pivot: HEEL,
                        fold: shape(24.0 + 15.0 * vigor, 15.0 + 9.0 * vigor),
                        contact: receiver_pitch,
                        peak: (0.088 + 0.067 * vigor) * scale,
                        rise_by: 0.47,
                        travel: [0.0, 1.0],
                    }
                    .sample(initial, t / LAUNCH_CATCH, math);
                } else if let Some(ball) = *front_ball {
                    ShoeTarget {
                        pivot: BALL,
                        point: ball,
                        clearance: 0.0,
                        turns: shape(0.0, 0.0).blend(
                            raw.turns,
                            smoothstep(LAUNCH_CATCH + 3.0, LAUNCH_CATCH + 6.0, t),
                            math,
                        ),
                    }
                } else {
                    ShoeTarget {
                        pivot: HEEL,
                        point: self.catch_heel,
                        clearance: 0.0,
                        turns: receiver_pitch.blend(
                            shape(0.0, 0.0),
                            smoothstep(LAUNCH_CATCH, LAUNCH_CATCH + 2.5, t),
                            math,
                        ),
                    }
                };
                let may_leave = if loaded {
                    t >= LAUNCH_CATCH + 1.0 && *gait + 0.5 >= *toe_off
                } else {
                    front_ball.is_some() && *gait >= *toe_off
                };
                if advance && following[side].is_none() && may_leave {
                    following[side] = displayed.map(|from| {
                        // The former plant can rejoin the run halfway through a high curl.
                        // Set the collection from the shoe we actually drew at departure.
                        let gap = (raw.sole.clearance() - from.sole.clearance()) / scale;
                        let collection = if loaded { smoothstep(0.090, 0.240, gap) } else { 0.0 };
                        FollowingGait { started: t, from, collection }
                    });
                }
                if let Some(gait) = following[side] {
                    let leave = t - gait.started;
                    let duration = gait.duration();
                    let progress = (leave / duration).clamp(0.0, 1.0);
                    let ease = smoothstep(0.0, duration, leave);
                    let rest = 1.0 - progress;
                    let travel = ease + (1.0 - rest * rest - ease) * gait.collection;
                    // Collect the heel over several frames while the advancing gait supplies
                    // the falling half of the arc and the next catch.
                    let lift =
                        smoothstep(-0.25 * (1.0 - gait.collection), 1.45 + 2.25 * gait.collection, leave);
                    let turns = ease + (lift - ease) * gait.collection;
                    ShoeTarget {
                        pivot: BALL,
                        point: gait.from.sole.ball().lerp(raw.sole.ball(), travel),
                        clearance: gait.from.sole.clearance().max(0.0) * (1.0 - lift)
                            + raw.sole.clearance() * lift,
                        turns: gait.from.turns.blend(raw.turns, turns, math),
                    }
                } else {
                    hold
                }
            }
        }
    }

    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.len(self.support);
        for shoe in self.initial {
            shoe.write_canonical(out);
        }
        out.floats(&self.facing.to_array());
        out.floats(&[self.scale, self.elapsed]);
        out.vec3(self.catch_heel);
        out.vec3(self.rear_ball);
        out.flag(matches!(self.action, Action::Launch { .. }));
        match self.action {
            Action::Brake { catch, transfer, vigor, late, already_arriving, close_from } => {
                out.floats(&[catch, transfer, vigor, late]);
                out.flag(already_arriving);
                out.optional(close_from, |out, shoe| shoe.write_canonical(out));
            }
            Action::Launch { speed, gait, toe_off, front_ball, following } => {
                out.floats(&[speed, gait, toe_off]);
                out.optional(front_ball, History::vec3);
                for gait in following {
                    out.optional(gait, |out, gait| {
                        out.floats(&[gait.started, gait.collection]);
                        gait.from.write_canonical(out);
                    });
                }
            }
        }
    }
}

struct Swing {
    target: Vec3,
    pivot: usize,
    fold: ShoeTurns,
    contact: ShoeTurns,
    peak: f32,
    rise_by: f32,
    travel: [f32; 2],
}

impl Swing {
    fn sample(&self, from: DisplayedShoe, phase: f32, math: PoseMath) -> ShoeTarget {
        let clearance = from.sole.clearance().max(0.0);
        let peak = self.peak.max(clearance + self.peak * 0.31);
        let rise = smoothstep(0.0, self.rise_by, phase);
        let drop = smoothstep(0.49, 1.0, phase);
        let travel = smoothstep(self.travel[0], self.travel[1], phase);
        let folded = from.turns.blend(self.fold, rise, math);
        ShoeTarget {
            pivot: self.pivot,
            point: from.sole.point(self.pivot).lerp(self.target, travel),
            clearance: (clearance + (peak - clearance) * rise) * (1.0 - drop),
            turns: folded.blend(self.contact, drop, math),
        }
    }

    fn descend(&self, from: DisplayedShoe, phase: f32, math: PoseMath) -> ShoeTarget {
        let arrival = smoothstep(0.0, 1.0, phase);
        ShoeTarget {
            pivot: self.pivot,
            point: from.sole.point(self.pivot).lerp(self.target, arrival),
            clearance: from.sole.clearance().max(0.0) * (1.0 - arrival),
            turns: from.turns.blend(self.contact, arrival, math),
        }
    }
}

fn track(side: usize) -> f32 {
    if side == 0 { 0.115 } else { -0.109 }
}

fn shoe_turns(facing: Quat, side: usize, pitch: f32, toe_pitch: f32, math: PoseMath) -> ShoeTurns {
    let yaw = if side == 0 { 7.0_f32 } else { -9.0_f32 };
    let forward = math.multiply(facing, math.axis_angle(Vec3::Y, yaw.to_radians()));
    let turn = |pitch: f32| math.multiply(forward, math.axis_angle(Vec3::X, pitch.to_radians()));
    ShoeTurns { shoe: turn(pitch), toes: turn(toe_pitch) }
}

fn hold(foot: DisplayedShoe, pivot: usize, turns: ShoeTurns, clearance: f32) -> ShoeTarget {
    ShoeTarget { pivot, point: foot.sole.point(pivot), clearance, turns }
}
