//! Match a clip's planted heel or ball to the ground that its warped root travels over.

use sapphire_math::ease::smoothstep;
use sapphire_math::{Affine3A, Vec3};

use super::gather::SoleHistory;
use crate::pose_math::PoseMath;

/// The root of a clip at the pose being drawn, before gameplay moves or turns it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AuthoredFootMotion {
    /// Which playing of the clip this is. The source motion starts over when this changes.
    pub serial: u32,
    /// Model space in the clip's own fixed start space, including the authored root curve.
    pub source_from_model: Affine3A,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum SoleMotion {
    Authored(AuthoredFootMotion),
    Landing(AuthoredFootMotion),
    LandingCatch(AuthoredFootMotion, f32),
    Gait,
    FreeStep,
    Inactive,
}

impl SoleMotion {
    pub(super) fn is_clip(self) -> bool {
        matches!(self, Self::Authored(_)) || self.is_landing()
    }

    pub(super) fn is_landing(self) -> bool {
        matches!(self, Self::Landing(_) | Self::LandingCatch(_, _))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Plant {
    pivot: usize,
    world: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Release {
    plant: Plant,
    elapsed: f32,
    lift: bool,
    fixed_planar: Option<(Vec3, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum LandingSwing {
    Leaving,
    Airborne,
}

/// Tracking history for one shoe. Both pivots are tested in source space, then the selected
/// pivot holds in world space even when a gameplay warp rotates the whole character.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct AuthoredSupport {
    previous: Option<SourceSole>,
    plant: Option<Plant>,
    release: Option<Release>,
    blocked: bool,
    landing: bool,
    swing: Option<LandingSwing>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SourceSole {
    serial: u32,
    authored: [Vec3; 2],
    world: [Vec3; 2],
}

#[derive(Clone, Copy)]
pub(super) struct SolePoints {
    pub model: [Vec3; 2],
    pub world: [Vec3; 2],
    pub solved: Option<SoleHistory>,
}

type History<'a, F> = crate::graph::canonical::Canonical<'a, F>;

impl AuthoredFootMotion {
    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.u32(self.serial);
        out.affine(self.source_from_model);
    }
}

impl Plant {
    fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.len(self.pivot);
        out.vec3(self.world);
    }
}

impl AuthoredSupport {
    pub(super) fn write_canonical(self, out: &mut History<'_, impl FnMut(&[u8])>) {
        out.optional(self.previous, |out, source| {
            out.u32(source.serial);
            for point in source.authored.into_iter().chain(source.world) {
                out.vec3(point);
            }
        });
        out.optional(self.plant, |out, plant| plant.write_canonical(out));
        out.optional(self.release, |out, release| {
            release.plant.write_canonical(out);
            out.f32(release.elapsed);
            out.flag(release.lift);
            out.optional(release.fixed_planar, |out, (offset, height)| {
                out.vec3(offset);
                out.f32(height);
            });
        });
        out.flag(self.blocked);
        out.flag(self.landing);
        out.optional(self.swing, |out, swing| out.flag(matches!(swing, LandingSwing::Airborne)));
    }
}
const CATCH_HEIGHT: f32 = 0.022;
const LIFT_HEIGHT: f32 = 0.042;
const RELEASE_SECONDS: f32 = 4.0 / 60.0;

impl AuthoredSupport {
    pub(super) fn active(&self) -> bool {
        self.plant.is_some() || self.release.is_some()
    }

    pub(super) fn landing_handoff(&self, motion: SoleMotion) -> bool {
        self.landing && matches!(motion, SoleMotion::Gait | SoleMotion::FreeStep)
    }

    pub(super) fn holds_landing_plant(&self) -> bool {
        self.landing && self.plant.is_some()
    }

    pub(super) fn settled_in(&self, motion: SoleMotion) -> bool {
        motion.is_clip() || !self.active() || (self.landing_handoff(motion) && self.release.is_none())
    }

    pub(super) fn update(
        &mut self,
        motion: SoleMotion,
        points: SolePoints,
        floor: f32,
        has_ground: bool,
        dt: f32,
        reach: impl Fn(Vec3) -> f32,
    ) {
        if let Some(release) = &mut self.release {
            release.elapsed += dt;
            if release.elapsed >= RELEASE_SECONDS {
                self.release = None;
            }
        }
        let SolePoints { model, world, solved } = points;
        let lift = model.map(|point| point.y - floor);
        let free_step = motion == SoleMotion::FreeStep;
        let leaving = free_step && self.landing && self.previous.is_some();
        if leaving {
            self.swing = Some(LandingSwing::Leaving);
        }
        if free_step && let Some(sole) = solved {
            match self.swing {
                Some(LandingSwing::Leaving) if sole.clearance() >= LIFT_HEIGHT => {
                    self.swing = Some(LandingSwing::Airborne);
                }
                Some(LandingSwing::Airborne) if has_ground => {
                    if let Some((pivot, point)) = sole.touchdown()
                        && lift[pivot] < LIFT_HEIGHT
                        && reach((point - world[pivot]).with_y(0.0)) >= 1.0
                    {
                        self.plant = Some(Plant { pivot, world: point });
                        self.release = None;
                        self.swing = None;
                        self.landing = true;
                    }
                }
                _ => {}
            }
        } else if !free_step {
            self.swing = None;
        }
        let handoff = self.landing_handoff(motion) && !leaving;
        self.landing = handoff || motion.is_landing();
        let fixed_share = if let SoleMotion::LandingCatch(_, share) = motion { share } else { 0.0 };
        let motion = match motion {
            SoleMotion::Authored(motion)
            | SoleMotion::Landing(motion)
            | SoleMotion::LandingCatch(motion, _) => Some(motion),
            SoleMotion::Gait | SoleMotion::FreeStep | SoleMotion::Inactive => None,
        };
        if (handoff || leaving)
            && self.previous.is_some()
            && let Some(plant) = &mut self.plant
        {
            if let Some(point) = solved.and_then(|sole| sole.grounded_point(plant.pivot, CATCH_HEIGHT)) {
                // Start from the displayed pivot, which can still be easing in from the air.
                plant.world = point;
            } else {
                self.plant = None;
            }
        }
        if motion.is_none() && !handoff {
            self.previous = None;
            self.blocked = false;
            // A shoe still on the floor needs clearance as it takes up the new swing.
            // An already airborne shoe continues the height authored for its step.
            self.release(leaving && self.plant.is_some());
            if leaving && let Some(release) = &mut self.release {
                let pivot = release.plant.pivot;
                // The outgoing shoe starts where it was drawn and takes up the new swing.
                // Fade the initial difference instead of chasing its old ground position.
                release.fixed_planar = Some(((release.plant.world - world[pivot]).with_y(0.0), 1.0));
            }
            return;
        }
        let source = motion.map(|motion| SourceSole {
            serial: motion.serial,
            authored: model.map(|point| motion.source_from_model.transform_point3(point)),
            world,
        });
        let previous = std::mem::replace(&mut self.previous, source);
        let same_clip = source.zip(previous).filter(|(source, prior)| source.serial == prior.serial);
        let travel = |pivot: usize| {
            same_clip.map_or(0.0, |(source, previous)| {
                (source.authored[pivot] - previous.authored[pivot]).with_y(0.0).length()
            })
        };
        // Tolerate the small drift from interpolating joint turns between keyed contacts.
        let stationary = |pivot| travel(pivot) <= 0.001 + 0.6 * dt;
        let catch = |pivot: usize| has_ground && lift[pivot] <= CATCH_HEIGHT && stationary(pivot);
        let lowest = usize::from(lift[1] < lift[0]);
        if same_clip.is_none() || (lift[lowest] >= LIFT_HEIGHT || travel(lowest) > 0.002 + 1.2 * dt) {
            self.blocked = false;
        }
        if let Some(plant) = self.plant {
            let pivot = plant.pivot;
            let other = 1 - pivot;
            let correction = (plant.world - world[pivot]).with_y(0.0);
            let moving = travel(pivot) > 0.002 + 1.15 * dt;
            if (lift[pivot] > CATCH_HEIGHT || moving) && catch(other) && lift[other] < lift[pivot] {
                // A planted shoe can roll from heel to ball. Keep the same translation across
                // the transfer, and let the new pivot pin the next part of the roll.
                self.plant = Some(Plant { pivot: other, world: world[other] + correction });
            } else {
                let lift_release =
                    has_ground && lift[pivot] < LIFT_HEIGHT && !moving && reach(correction) < 1.0;
                if !has_ground || lift[pivot] >= LIFT_HEIGHT || moving || lift_release {
                    self.blocked |= lift_release;
                    self.release(lift_release);
                    if fixed_share > 0.0
                        && let Some((_, prior)) = same_clip
                        && let Some(release) = &mut self.release
                    {
                        // The shoe is already leaving. Keep the offset achieved on the last
                        // displayed plant while it follows the authored swing from that point.
                        let displayed = solved
                            .and_then(|sole| sole.grounded_point(pivot, LIFT_HEIGHT))
                            .unwrap_or(plant.world);
                        release.fixed_planar =
                            Some(((displayed - prior.world[pivot]).with_y(0.0), fixed_share));
                    }
                }
            }
        }
        if motion.is_some() && self.plant.is_none() && self.release.is_none() && !self.blocked {
            let pivot = if catch(lowest) { Some(lowest) } else { catch(1 - lowest).then_some(1 - lowest) };
            if let Some(pivot) = pivot {
                self.plant = Some(Plant { pivot, world: world[pivot] });
            }
        }
    }

    /// Translational correction at the ankle in world space. `reach` gives the reachable
    /// fraction of a horizontal correction and accounts for the lifted shoe during release.
    pub(super) fn correction(
        &self,
        world: [Vec3; 2],
        source_lift: f32,
        reach: impl Fn(Vec3, f32) -> f32,
        math: PoseMath,
    ) -> Vec3 {
        let (planar, amount, lift) = if let Some(plant) = self.plant {
            ((plant.world - world[plant.pivot]).with_y(0.0), 1.0, 0.0)
        } else if let Some(release) = self.release {
            let phase = release.elapsed / RELEASE_SECONDS;
            let lift = if release.lift {
                (0.055 - source_lift.max(0.0)).max(0.0) * math.sin(std::f32::consts::PI * phase)
            } else {
                0.0
            };
            let tracking = || (release.plant.world - world[release.plant.pivot]).with_y(0.0);
            let planar = match release.fixed_planar {
                Some((fixed, 1.0)) => fixed,
                Some((fixed, share)) => tracking().lerp(fixed, share),
                None => tracking(),
            };
            (planar, 1.0 - smoothstep(0.0, 1.0, phase), lift)
        } else {
            return Vec3::ZERO;
        };
        let planar = planar * amount;
        (planar * reach(planar, lift)).with_y(lift)
    }

    fn release(&mut self, lift: bool) {
        if let Some(plant) = self.plant.take() {
            self.release = Some(Release { plant, elapsed: 0.0, lift, fixed_planar: None });
        }
    }
}

/// The share of a displacement that a leg can reach before it loses its flexion reserve.
pub(super) fn reachable_share(ankle_from_hip: Vec3, displacement: Vec3, length: f32) -> f32 {
    let a = displacement.length_squared();
    if a <= 1e-10 || (ankle_from_hip + displacement).length_squared() <= length * length {
        return 1.0;
    }
    let b = ankle_from_hip.dot(displacement);
    let c = ankle_from_hip.length_squared() - length * length;
    ((-b + (b * b - a * c).max(0.0).sqrt()) / a).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEP: f32 = 1.0 / 60.0;

    fn motion(root: Vec3) -> SoleMotion {
        SoleMotion::Authored(AuthoredFootMotion {
            serial: 1,
            source_from_model: Affine3A::from_translation(root),
        })
    }

    fn landing() -> SoleMotion {
        SoleMotion::Landing(AuthoredFootMotion { serial: 2, source_from_model: Affine3A::IDENTITY })
    }

    fn points(authored: Vec3, warp: Vec3) -> SolePoints {
        let model = [-authored, Vec3::new(0.0, 0.08, 0.14) - authored];
        let world = model.map(|point| point + authored + warp);
        SolePoints { model, world, solved: None }
    }

    #[test]
    fn a_stationary_source_heel_holds_even_as_the_actual_root_moves() {
        let mut foot = AuthoredSupport::default();
        let advance = Vec3::Z * 0.02;
        let warp = Vec3::new(0.13, 0.0, 0.31);
        foot.update(motion(Vec3::ZERO), points(Vec3::ZERO, Vec3::ZERO), 0.0, true, STEP, |_| 1.0);
        let moved = points(advance, warp);
        foot.update(motion(advance), moved, 0.0, true, STEP, |_| 1.0);
        let correction = foot.correction(moved.world, 0.0, |_, _| 1.0, PoseMath::Legacy);
        assert!((moved.world[0] + correction).abs_diff_eq(Vec3::ZERO, 1e-5));
        assert_eq!(
            foot.correction(points(advance, Vec3::ZERO).world, 0.0, |_, _| 1.0, PoseMath::Legacy),
            Vec3::ZERO
        );
    }

    #[test]
    fn an_authored_swing_releases_then_catches_at_its_new_world_contact() {
        let mut foot = AuthoredSupport::default();
        let start = points(Vec3::ZERO, Vec3::ZERO);
        foot.update(motion(Vec3::ZERO), start, 0.0, true, STEP, |_| 1.0);
        let flight = SolePoints {
            model: start.model.map(|point| point + Vec3::Y * 0.11),
            world: start.world.map(|point| point + Vec3::Y * 0.11 + Vec3::Z * 0.25),
            solved: None,
        };
        for _ in 0..6 {
            foot.update(motion(Vec3::ZERO), flight, 0.0, true, STEP, |_| 1.0);
        }
        assert!(!foot.active());
        assert_eq!(foot.correction(flight.world, 0.11, |_, _| 1.0, PoseMath::Legacy), Vec3::ZERO);
        let catch = points(Vec3::ZERO, Vec3::Z * 0.4);
        foot.update(motion(Vec3::ZERO), catch, 0.0, true, STEP, |_| 1.0);
        let warped = points(Vec3::ZERO, Vec3::Z * 0.52);
        foot.update(motion(Vec3::ZERO), warped, 0.0, true, STEP, |_| 1.0);
        assert!(
            (warped.world[0] + foot.correction(warped.world, 0.0, |_, _| 1.0, PoseMath::Legacy))
                .abs_diff_eq(catch.world[0], 1e-5)
        );
    }

    #[test]
    fn accelerating_catch_releases_from_the_last_displayed_plant() {
        let mut foot = AuthoredSupport::default();
        let SoleMotion::Landing(motion) = landing() else { unreachable!() };
        let catch = SoleMotion::LandingCatch(motion, 1.0);
        foot.update(landing(), points(Vec3::ZERO, Vec3::ZERO), 0.0, true, STEP, |_| 1.0);
        let last = points(Vec3::ZERO, Vec3::Z * 0.04);
        foot.update(catch, last, 0.0, true, STEP, |_| 1.0);
        let flight = SolePoints {
            model: last.model.map(|point| point + Vec3::Y * 0.08 + Vec3::Z * 0.16),
            world: last.world.map(|point| point + Vec3::Y * 0.08 + Vec3::Z * 0.16),
            solved: None,
        };
        foot.update(catch, flight, 0.0, true, STEP, |_| 1.0);
        let correction = foot.correction(flight.world, 0.08, |_, _| 1.0, PoseMath::Legacy);
        assert!(correction.abs_diff_eq(-Vec3::Z * 0.04, 1e-5));
        assert!(((flight.world[0] + correction).z - 0.16).abs() < 1e-5);
    }

    #[test]
    fn losing_reach_lifts_the_foot_and_does_not_replant_a_stale_contact() {
        let mut foot = AuthoredSupport::default();
        let start = points(Vec3::ZERO, Vec3::ZERO);
        let warped = points(Vec3::ZERO, Vec3::Z * 0.5);
        foot.update(motion(Vec3::ZERO), start, 0.0, true, STEP, |_| 1.0);
        foot.update(motion(Vec3::ZERO), warped, 0.0, true, STEP, |_| 0.5);
        foot.update(motion(Vec3::ZERO), warped, 0.0, true, STEP, |_| 0.5);
        let correction = foot.correction(warped.world, 0.0, |_, _| 0.5, PoseMath::Legacy);
        assert!(correction.y > 0.03);
        assert!(correction.z > -0.25);
        for _ in 0..6 {
            foot.update(motion(Vec3::ZERO), warped, 0.0, true, STEP, |_| 1.0);
        }
        assert!(!foot.active());
    }

    #[test]
    fn a_grounded_landing_pivot_carries_into_gait_until_toe_off() {
        let mut foot = AuthoredSupport::default();
        let start = points(Vec3::ZERO, Vec3::ZERO);
        foot.update(landing(), start, 0.0, true, STEP, |_| 1.0);
        let visible = start.world.map(|point| point + Vec3::Z * 0.03);
        let mut next = points(Vec3::ZERO, Vec3::Z * 0.18);
        next.solved = Some(SoleHistory::new(visible, [0.0; 2], None, STEP));
        foot.update(SoleMotion::Gait, next, 0.0, true, STEP, |_| 1.0);
        next.solved = None;
        for _ in 0..6 {
            next.world = next.world.map(|point| point + Vec3::Z * 0.03);
            foot.update(SoleMotion::Gait, next, 0.0, true, STEP, |_| 1.0);
            let correction = foot.correction(next.world, 0.0, |_, _| 1.0, PoseMath::Legacy);
            assert!((next.world[0] + correction).abs_diff_eq(visible[0], 1e-5));
        }
        next.model = next.model.map(|point| point + Vec3::Y * 0.06);
        next.world = next.world.map(|point| point + Vec3::Y * 0.06);
        for _ in 0..6 {
            foot.update(SoleMotion::Gait, next, 0.0, true, STEP, |_| 1.0);
        }
        assert!(!foot.active());
        foot.update(SoleMotion::Gait, start, 0.0, true, STEP, |_| 1.0);
        assert!(!foot.active(), "the landing policy only carries a prior plant");
    }

    #[test]
    fn landing_handoff_requires_a_grounded_last_solved_pivot() {
        let start = points(Vec3::ZERO, Vec3::ZERO);
        let hovering = start.world.map(|point| point + Vec3::Y * 0.07);
        let mut next = points(Vec3::ZERO, Vec3::Z * 0.18);
        next.solved = Some(SoleHistory::new(hovering, [0.0; 2], None, STEP));
        for motion in [SoleMotion::Gait, SoleMotion::FreeStep] {
            let mut foot = AuthoredSupport::default();
            foot.update(landing(), start, 0.0, true, STEP, |_| 1.0);
            foot.update(motion, next, 0.0, true, STEP, |_| 1.0);
            assert!(!foot.active());
        }
    }

    #[test]
    fn the_unloaded_landing_shoe_releases_from_its_last_visible_position() {
        let mut foot = AuthoredSupport::default();
        let start = points(Vec3::ZERO, Vec3::ZERO);
        foot.update(landing(), start, 0.0, true, STEP, |_| 1.0);
        let visible = start.world.map(|point| point + Vec3::Z * 0.03);
        let mut next = points(Vec3::ZERO, Vec3::Z * 0.18);
        next.solved = Some(SoleHistory::new(visible, [0.0; 2], None, STEP));
        foot.update(SoleMotion::FreeStep, next, 0.0, true, STEP, |_| 1.0);
        let correction = foot.correction(next.world, 0.0, |_, _| 1.0, PoseMath::Legacy);
        assert!((next.world[0] + correction).abs_diff_eq(visible[0], 1e-5));
        next.world = next.world.map(|point| point + Vec3::Z * 0.20);
        foot.update(SoleMotion::FreeStep, next, 0.0, true, STEP, |_| 1.0);
        let departing = foot.correction(next.world, 0.0, |_, _| 1.0, PoseMath::Legacy);
        assert!(departing.with_y(0.0).length() < correction.length());
        assert!(departing.y > 0.03, "the shoe clears the floor as it steps forward");
        assert_eq!(foot.correction(next.world, 0.065, |_, _| 1.0, PoseMath::Legacy).y, 0.0);
        for _ in 0..5 {
            foot.update(SoleMotion::FreeStep, next, 0.0, true, STEP, |_| 1.0);
        }
        assert!(!foot.active());
    }

    #[test]
    fn a_free_landing_shoe_needs_clearance_then_a_slow_solved_contact_to_plant_again() {
        let mut foot = AuthoredSupport::default();
        let mut start = points(Vec3::ZERO, Vec3::ZERO);
        let settled = SoleHistory::new(start.world, [0.0; 2], None, STEP);
        start.solved = Some(settled);
        foot.update(landing(), start, 0.0, true, STEP, |_| 1.0);
        for _ in 0..3 {
            foot.update(SoleMotion::FreeStep, start, 0.0, true, STEP, |_| 1.0);
        }
        assert!(foot.plant.is_none(), "initial double support is not a new catch");

        let raised = start.world.map(|point| point + Vec3::Y * 0.09);
        start.solved = Some(SoleHistory::new(raised, [0.0; 2], Some(settled), STEP));
        foot.update(SoleMotion::FreeStep, start, 0.0, true, STEP, |_| 1.0);
        let new_contact = start.world.map(|point| point + Vec3::Z * 0.30);
        let moving = SoleHistory::new(new_contact, [0.0; 2], start.solved, STEP);
        start.solved = Some(moving);
        foot.update(SoleMotion::FreeStep, start, 0.0, true, STEP, |_| 1.0);
        assert!(foot.plant.is_none(), "the sole is still moving along the ground");

        let mut next = points(Vec3::ZERO, Vec3::Z * 0.40);
        next.solved = Some(SoleHistory::new(new_contact, [0.0; 2], Some(moving), STEP));
        foot.update(SoleMotion::FreeStep, next, 0.0, true, STEP, |_| 1.0);
        let correction = foot.correction(next.world, 0.0, |_, _| 1.0, PoseMath::Legacy);
        assert!((next.world[0] + correction).abs_diff_eq(new_contact[0], 1e-5));
    }

    #[test]
    fn a_release_stays_within_the_leg_length() {
        let base = Vec3::new(0.0, -0.66, 0.0);
        let adjustment = Vec3::Z * 0.6;
        let share = reachable_share(base, adjustment, 0.76);
        assert!((0.0..1.0).contains(&share));
        assert!(((base + adjustment * share).length() - 0.76).abs() < 1e-5);
    }
}
