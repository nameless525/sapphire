//! The brief moving load uses the shoe that actually meets the ground.

use sapphire_math::{Affine3A, Quat, Vec3};

use super::{
    FeetInput, FootContact, FootPlacement, FootProbe, History, LegBones, Plane, smoothstep, supporting,
};
use crate::ik::footwork::{DisplayedShoe, ShoeTurns};
use crate::ik::solve::SolvePose;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TakeoffSupport {
    pub(super) side: usize,
    pivot: usize,
    points: [Vec3; 2],
}

impl TakeoffSupport {
    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.len(self.side);
        out.len(self.pivot);
        for point in self.points {
            out.vec3(point);
        }
    }
}

impl FootPlacement {
    pub(super) fn update_takeoff_support(
        &mut self,
        pose: &SolvePose<'_>,
        input: &FeetInput,
        probes: &[FootProbe; 2],
        place: &Affine3A,
    ) {
        if self.takeoff_phase.is_none()
            || self.fade_goal == 0.0
            || !matches!(
                self.contact,
                FootContact::Stance | FootContact::RollingStance | FootContact::Launch { .. }
            )
        {
            self.takeoff_support = None;
            return;
        }
        if self.takeoff_support.is_some() {
            return;
        }
        let mut best = None;
        let mut score = f32::INFINITY;
        for (side, leg) in self.legs.iter().enumerate() {
            if self.footwork.is_some_and(|step| step.controls(side)) {
                continue;
            }
            let Some(plane) = supporting(&input.ground[side], &probes[side], self.settings.min_normal_y)
            else {
                continue;
            };
            let sole = leg.sole(pose, true).map(|point| place.transform_point3(point));
            let gaps = sole.map(|point| point.y - plane.height_at(point));
            let pivot = usize::from(gaps[1] < gaps[0]);
            let gap = gaps[pivot];
            let previous = self.feet[side].last_sole;
            // A running stride can be airborne on the press. Let it take its next catch
            // from the incoming descending shoe. An already rising shoe keeps swinging.
            let approaching = previous.is_some_and(|last| last.clearance() > gap + 0.002);
            let limit = if approaching { 0.022 } else { 0.010 };
            if !(-0.045..=limit).contains(&gap) {
                continue;
            }
            let grounded = previous
                .and_then(|last| last.touchdown())
                .filter(|&(last_pivot, point)| {
                    let travel = (sole[pivot] - point).with_y(0.0).length();
                    last_pivot == pivot && travel < 0.07
                })
                .map(|(_, point)| point.with_y(plane.height_at(point)));
            let rank = gap.abs() - if grounded.is_some() { 0.02 } else { 0.0 };
            if rank < score {
                score = rank;
                let mut points = sole.map(|point| point.with_y(plane.height_at(point)));
                if let Some(point) = grounded {
                    let shift = point - points[pivot];
                    points = points.map(|point| {
                        let shifted = point + shift;
                        shifted.with_y(plane.height_at(shifted))
                    });
                }
                best = Some(TakeoffSupport { side, pivot, points });
            }
        }
        self.takeoff_support = best;
    }
}

impl TakeoffSupport {
    /// Return the corrected ankle target after rolling through the ball about the ground plane.
    pub(super) fn push(
        self,
        placement: &FootPlacement,
        pose: &mut SolvePose<'_>,
        place: &Affine3A,
        plane: Plane,
        phase: f32,
        model_up: Vec3,
    ) -> Option<(Vec3, f32)> {
        let leg = &placement.legs[self.side];
        let math = pose.math();
        let ankle = leg.chain.end;
        let toes = leg.toes?;
        let rest_toes = leg.rest_toes?;
        let toe_rotation = pose.rotation(usize::from(toes));
        let turn = math.multiply(toe_rotation, rest_toes.inverse());
        let forward = math.rotate(turn, Vec3::Z);
        let tangent = (forward - model_up * forward.dot(model_up)).normalize_or(forward);
        let lateral = model_up.cross(tangent).normalize_or(Vec3::X);
        let push = smoothstep(phase);
        let current = leg.sole(pose, true).map(|point| place.transform_point3(point));
        let support = self.points[self.pivot].lerp(self.points[1], push);
        let pivot = current[self.pivot].lerp(current[1], push);
        if (support - pivot).with_y(0.0).length() > 0.145 {
            return None;
        }
        if push > 0.0 {
            let heel_roll = math.axis_angle(lateral, 13.0_f32.to_radians() * push);
            pose.set_rotation(ankle, math.multiply(heel_roll, pose.rotation(ankle)));
            pose.refresh(&leg.path);
            let align = math.rotation_arc(forward, tangent);
            let flat_toe = math.multiply(align, toe_rotation);
            pose.set_rotation(usize::from(toes), flat_toe);
            pose.refresh(&[toes]);
        }
        let soles = leg.sole(pose, true).map(|point| place.transform_point3(point));
        let pivot = soles[self.pivot].lerp(soles[1], push);
        let floor = soles.map(|point| plane.height_at(point) - point.y);
        // The forefoot mesh extends below the toe pivot as weight rolls through the tip.
        let scale = place.matrix3.y_axis.length();
        let tip_room = leg.ball_height.unwrap_or(0.0) * scale * 0.28 * push;
        let target = (support - pivot).with_y((support.y - pivot.y).max(floor[0].max(floor[1])) + tip_room);
        let load = -0.022 + push * 0.033;
        Some((target, load))
    }
}

/// An authored stance can move the solved shoes away from the graph's previous gait sample.
/// Retain both displayed shoe transforms as they lift away from that stance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct TakeoffCarry {
    shoes: [DisplayedShoe; 2],
    elapsed: f32,
}

impl TakeoffCarry {
    const JOIN_TICKS: f32 = 4.0;

    pub(super) fn begin(shoes: [Option<DisplayedShoe>; 2]) -> Option<Self> {
        Some(Self { shoes: [shoes[0]?, shoes[1]?], elapsed: 0.0 })
    }

    pub(super) fn advance(&mut self, dt: f32) -> bool {
        self.elapsed += dt * 60.0;
        self.elapsed < Self::JOIN_TICKS
    }

    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        for shoe in self.shoes {
            shoe.sole.write_canonical(out);
            shoe.turns.write_canonical(out);
        }
        out.f32(self.elapsed);
    }

    pub(super) fn ankle_offset(
        self,
        side: usize,
        leg: &LegBones,
        pose: &mut SolvePose<'_>,
        place: &Affine3A,
        world_turn: Quat,
        scale: f32,
        raw_offset: Vec3,
    ) -> Vec3 {
        let math = pose.math();
        let model_turn = world_turn.inverse();
        let previous = self.shoes[side];
        let raw_turns = leg.shoe_turns(pose, world_turn);
        let pivot = previous.sole.low_pivot();
        let raw = place.transform_point3(leg.sole(pose, true)[pivot] + raw_offset);
        let last = previous.sole.point(pivot);
        let join = smoothstep((self.elapsed - 0.5) / (Self::JOIN_TICKS - 0.5));
        // A loaded toe releases close to its old world point. The already swinging shoe
        // travels farther, without inheriting the ground pin or its next gait target.
        let planted = previous.sole.clearance() <= 0.025 * scale;
        let travel = if planted { 0.032 } else { 0.058 };
        let time_scale = scale * self.elapsed;
        let planar = (raw - last).with_y(0.0).clamp_length_max(travel * time_scale);
        let rise =
            (raw.y - last.y).clamp(if planted { 0.040 } else { -0.016 } * time_scale, 0.105 * time_scale);
        let departed = last + planar + Vec3::Y * rise;
        let target = departed.lerp(raw, join);
        let turns = ShoeTurns {
            shoe: math.slerp(previous.turns.shoe, raw_turns.shoe, join),
            toes: math.slerp(previous.turns.toes, raw_turns.toes, join),
        };
        leg.set_shoe_turns(pose, turns, model_turn);
        pose.refresh(&leg.path);
        let rolled = place.transform_point3(leg.sole(pose, true)[pivot]);
        math.rotate(model_turn, target - rolled) / scale
    }
}
