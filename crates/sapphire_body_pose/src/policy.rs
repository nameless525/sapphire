//! Stateful body policy shared by callers that supply the graph clock and terrain.
use sapphire_core::Name;
use sapphire_math::{Affine3A, Mat4, Vec3, ease::smoothstep};

use crate::pose_math::PoseMath;

use crate::{AuthoredFootMotion, CharacterPose, ClipSource, FootContact, GraphInput, GraphPlayer};

/// The graph's contact priority before the caller falls back to its normal stance mode.
pub fn graph_foot_contact(
    player: &GraphPlayer,
    clips: &(impl ClipSource + ?Sized),
    place: Mat4,
    turning: Option<FootContact>,
) -> Option<FootContact> {
    player
        .ground_foot_contact()
        .or_else(|| player.grounded_gather().map(|phase| FootContact::Gather { phase }))
        .or_else(|| player.landing_support().map(|side| FootContact::LandingStep { side }))
        .or_else(|| {
            let (serial, root) = player.landing_root(clips)?;
            let scale = place.y_axis.truncate().length();
            let source_from_model = Affine3A::from_scale_rotation_translation(
                root.scale * scale,
                root.rotation,
                root.translation * scale,
            );
            let motion = AuthoredFootMotion { serial, source_from_model };
            let release = player.accelerating_landing_weight();
            Some(if release > 0.0 {
                FootContact::LandingCatch { motion, release }
            } else {
                FootContact::Landing(motion)
            })
        })
        .or(turning)
}

#[derive(Clone, Copy, Debug)]
struct HeadLookSample {
    ticks: f64,
    serial: Option<u32>,
    retained: f32,
}

/// The current source envelope and any authored head control still leaving a previous move.
#[derive(Clone, Copy, Debug)]
pub struct AuthoredHeadLook {
    movement: Name,
    retained: f32,
    source_ticks: [f32; 4],
    last: Option<HeadLookSample>,
    outgoing: Option<(f64, f32)>,
}

impl AuthoredHeadLook {
    /// Starts an authored neck/head control envelope, leaving eyes under the existing IK policy.
    pub fn new(movement: Name, retained: f32, source_ticks: [f32; 4]) -> Self {
        Self { movement, retained: retained.clamp(0.0, 1.0), source_ticks, last: None, outgoing: None }
    }

    /// Move whose authored source position drives the envelope.
    pub fn movement(&self) -> Name {
        self.movement
    }

    /// Clears retained state for an incompatible character replacement.
    pub fn reset(&mut self) {
        self.last = None;
        self.outgoing = None;
    }

    /// Returns the remaining procedural neck/head weight at the given presented owner time.
    pub fn weight(&mut self, ticks: f64, sample: Option<(u32, f32)>, cut: bool) -> f32 {
        const EXIT_TICKS: f32 = 6.0;
        let ticks = if ticks.is_finite() { ticks } else { self.last.map_or(0.0, |last| last.ticks) };
        let [start, held, release, end] = self.source_ticks;
        let (serial, mut retained) = sample.map_or((None, 0.0), |(serial, position)| {
            let rise = smoothstep(start, held, position);
            let fall = smoothstep(end, release, position);
            (Some(serial), self.retained * rise * fall)
        });
        if let Some(last) = self.last.filter(|last| !cut && ticks >= last.ticks) {
            if ticks == last.ticks {
                return 1.0 - last.retained;
            }
            if serial != last.serial {
                self.outgoing = (last.retained > 0.0).then_some((last.ticks, last.retained));
            }
            if let Some((started, weight)) = self.outgoing {
                let elapsed = (ticks - started) as f32;
                let carry = weight * smoothstep(EXIT_TICKS, 0.0, elapsed);
                retained += (1.0 - retained) * carry;
            }
        } else {
            self.outgoing = None;
        }
        self.last = Some(HeadLookSample { ticks, serial, retained });
        1.0 - retained
    }
}

/// One turn step is triggered by a rapid change of presented facing. The root and all action
/// decisions still come from the controller. The chest catches up after the hips begin to turn.
#[derive(Clone, Debug, Default)]
pub struct GroundTurn {
    previous: Option<(f64, f32, f32)>,
    started: Option<f64>,
    steering: bool,
    from_gait: bool,
    gathered: bool,
    matches_next_step: bool,
    twist: f32,
}

type History<'a, F> = crate::graph::canonical::Canonical<'a, F>;

impl AuthoredHeadLook {
    pub(crate) fn write_canonical(&self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.optional(self.last, |out, last| {
            out.f64(last.ticks);
            out.optional(last.serial, History::u32);
            out.f32(last.retained);
        });
        out.optional(self.outgoing, |out, (ticks, retained)| {
            out.f64(ticks);
            out.f32(retained);
        });
    }
}

impl GroundTurn {
    pub(crate) fn write_canonical(&self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.optional(self.previous, |out, (time, heading, speed)| {
            out.f64(time);
            out.floats(&[heading, speed]);
        });
        out.optional(self.started, History::f64);
        for flag in [self.steering, self.from_gait, self.gathered, self.matches_next_step] {
            out.flag(flag);
        }
        out.f32(self.twist);
    }
}
impl GroundTurn {
    /// Determines graph receiving foot and contact policy before the next body evaluation.
    pub fn prepare(
        &mut self,
        player: &GraphPlayer,
        clips: &(impl ClipSource + ?Sized),
        input: &GraphInput,
        place: Mat4,
        support: impl FnOnce() -> Option<usize>,
    ) -> (Option<FootContact>, Option<usize>) {
        let math = player.pose().math();
        let forward = place.z_axis.truncate();
        let heading = math.atan2(forward.x, forward.z);
        let eligible = player.landing_root(clips).is_none() && player.landing_support().is_none();
        let (phase, rejoin) = self.update(input, heading, eligible, math);
        let receiving = if rejoin { support().map(|side| 1 - side) } else { None };
        (phase.map(|phase| FootContact::TurnStep { phase }), receiving)
    }

    /// Saves the current support route and applies the turn's chest lag before IK.
    pub fn apply_spine(
        &mut self,
        pose: &mut CharacterPose,
        spine: Option<usize>,
        contact: Option<FootContact>,
    ) {
        self.gathered = matches!(contact, Some(FootContact::Gather { .. } | FootContact::Brake { .. }));
        if let Some(spine) = spine.filter(|_| self.twist != 0.0) {
            let math = pose.math();
            let axis = pose
                .parent(spine)
                .map_or(Vec3::Y, |parent| math.rotate(pose.rest_rotation(parent).inverse(), Vec3::Y));
            let local = &mut pose.locals_mut()[spine];
            local.rotation = math.multiply(math.axis_angle(axis, self.twist), local.rotation);
        }
    }

    fn update(
        &mut self,
        input: &GraphInput,
        heading: f32,
        eligible: bool,
        math: PoseMath,
    ) -> (Option<f32>, bool) {
        const STEP_TICKS: f64 = 9.0;
        let now = input.ticks;
        let speed = input.locomotion.speed;
        let sample = (now, heading, speed);
        let eligible = eligible
            && input.locomotion.grounded
            && !input.locomotion.landed
            && input.combat.is_none()
            && !input.defeated;
        let previous = self.previous;
        if input.cut || previous.is_none_or(|(then, _, _)| now < then) {
            *self = Self { previous: Some(sample), ..Self::default() };
            return (None, false);
        }
        let (then, old_heading, old_speed) = previous.unwrap_or(sample);
        let dt = now - then;
        if dt > 0.0 {
            self.previous = Some(sample);
            let delta = heading - old_heading;
            let delta = math.atan2(math.sin(delta), math.cos(delta));
            let steering = eligible && speed >= 0.4 && delta.abs() >= 0.12 * dt as f32;
            let brakes_into_turn = old_speed >= 1.6 && old_speed - speed >= 0.35 * dt as f32;
            let starts_sideways = old_speed <= 0.4 && input.locomotion.direction.abs() >= 0.6;
            if steering && !self.steering && self.started.is_none() && (brakes_into_turn || starts_sideways) {
                self.started = Some(then);
                self.from_gait = brakes_into_turn;
                self.matches_next_step = brakes_into_turn || self.gathered;
            }
            self.steering = eligible && delta.abs() >= 0.12 * dt as f32;
            let offset = if self.steering && self.started.is_some() { delta } else { 0.0 };
            self.twist = ((self.twist - offset) * math.powf(0.5, dt as f32 / 2.5)).clamp(-0.38, 0.38);
            if self.twist.abs() < 0.001 {
                self.twist = 0.0;
            }
        }
        self.started = self.started.filter(|&start| eligible && now - start <= STEP_TICKS * 1.45);
        let phase = self.started.map(|start| ((now - start) / STEP_TICKS) as f32);
        let ready = if self.from_gait {
            phase.is_some_and(|phase| phase >= 0.8) && speed >= 1.8 && speed > old_speed
        } else {
            phase.is_some_and(|phase| phase >= 0.65) && speed >= 1.5
        };
        let rejoin = self.matches_next_step && ready;
        self.matches_next_step &= !rejoin;
        (phase, rejoin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CombatInput;

    const DODGE: Name = Name::new("dodge");

    fn head_look() -> AuthoredHeadLook {
        AuthoredHeadLook {
            movement: Name::new("light_3"),
            retained: 0.82,
            source_ticks: [7.0, 11.0, 16.0, 22.0],
            last: None,
            outgoing: None,
        }
    }

    #[test]
    fn authored_head_look_follows_fractional_source_ticks_and_holds_during_hitstop() {
        let mut look = head_look();
        let close = |actual: f32, expected: f32| assert!((actual - expected).abs() < 1e-5);
        close(look.weight(10.0, Some((1, 7.0)), false), 1.0);
        close(look.weight(10.25, Some((1, 7.5)), false), 0.964_765_6);
        close(look.weight(11.0, Some((1, 9.0)), false), 0.59);
        close(look.weight(12.0, Some((1, 11.0)), false), 0.18);
        close(look.weight(12.0, None, false), 0.18);
        close(look.weight(13.0, Some((1, 16.0)), false), 0.18);
        close(look.weight(14.0, Some((1, 19.0)), false), 0.59);
        close(look.weight(15.0, Some((1, 22.0)), false), 1.0);
    }

    #[test]
    fn an_interrupted_head_look_releases_and_a_cut_starts_from_the_current_clip() {
        let mut look = head_look();
        let close = |actual: f32, expected: f32| assert!((actual - expected).abs() < 1e-5);
        close(look.weight(40.0, Some((1, 12.0)), false), 0.18);
        close(look.weight(40.0, None, false), 0.18);
        let releasing = look.weight(41.0, None, false);
        assert!((0.18..0.3).contains(&releasing));
        let reentry = look.weight(42.0, Some((2, 0.0)), false);
        assert!((releasing..0.4).contains(&reentry));
        close(look.weight(47.0, Some((2, 5.0)), false), 1.0);
        close(look.weight(48.0, Some((2, 11.0)), false), 0.18);
        close(look.weight(48.0, None, true), 1.0);
        close(look.weight(30.0, Some((3, 16.0)), false), 0.18);
        close(look.weight(29.0, None, false), 1.0);
    }

    #[test]
    fn a_fast_reversal_selects_one_receiving_step_and_yields_to_combat() {
        let mut turn = GroundTurn::default();
        let mut input = GraphInput::default();
        input.locomotion.speed = 6.0;
        let mut heading = 3.0;
        assert_eq!(turn.update(&input, heading, true, PoseMath::Legacy), (None, false));
        for (tick, speed) in [5.1, 4.2, 3.3, 2.4, 1.5, 0.6, 0.3, 1.2, 2.1].into_iter().enumerate() {
            input.ticks = (tick + 1) as f64;
            input.locomotion.speed = speed;
            heading = (heading + 0.21 + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU)
                - std::f32::consts::PI;
            let (phase, rejoin) = turn.update(&input, heading, true, PoseMath::Legacy);
            assert!(phase.is_some());
            assert_eq!(rejoin, tick == 8);
        }
        input.ticks += 1.0;
        input.combat = Some(CombatInput { name: DODGE, serial: 1, position: 0.0 });
        assert_eq!(turn.update(&input, heading + 0.21, true, PoseMath::Legacy), (None, false));
        input.cut = true;
        turn.update(&input, heading, true, PoseMath::Legacy);
        assert_eq!(turn.twist, 0.0);
    }

    #[test]
    fn a_small_steering_adjustment_does_not_start_a_turn_step() {
        let mut turn = GroundTurn::default();
        let mut input = GraphInput::default();
        turn.update(&input, 0.0, true, PoseMath::Legacy);
        input.ticks = 1.0;
        input.locomotion.speed = 0.9;
        input.locomotion.direction = 0.2;
        assert_eq!(turn.update(&input, 0.2, true, PoseMath::Legacy), (None, false));
        assert_eq!(turn.twist, 0.0);
    }
}
