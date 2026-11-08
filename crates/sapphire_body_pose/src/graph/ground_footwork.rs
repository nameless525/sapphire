//! Hero starts and stops. The key poses author the body above the legs. The matching footwork
//! owns the two world-space sole paths and the clock on which the receiving gait begins.

use sapphire_assets::skeleton::ArchivedSkeleton;
use sapphire_math::ease::smoothstep;

use super::{
    ClipSource, Frame, GraphBinding, GraphInput, GraphPlayer, JointMask, Moment, Motion, PoseMath, Transform,
    blend_with_math, clip_position, sample, velocity_of,
};
use crate::{CharacterPose, FootContact};

pub(crate) const LAUNCH_CATCH: f32 = 8.0;
pub(crate) const LAUNCH_LIMIT: f32 = 40.0;
pub(crate) const BRAKE_SETTLED: f32 = 20.0;

#[derive(Clone, Debug)]
pub(super) struct FootworkClips {
    clips: [[u16; 2]; 2],
    forward_gaits: [(u16, f32); 3],
    mask: JointMask,
}

impl FootworkClips {
    pub(super) fn bind(
        clip: impl Fn(&str) -> Option<u16>,
        skeleton: &ArchivedSkeleton,
        hips: usize,
        spine: usize,
    ) -> Option<Self> {
        let clips =
            [[clip("brake_left")?, clip("brake_right")?], [clip("launch_left")?, clip("launch_right")?]];
        // The ball stays down through these authored ticks in locomotion.py. Each cycle
        // uses its own contact duration, including walk's overlap and run's flight.
        let forward_gaits =
            [(clip("walk")?, 24.0 / 42.0), (clip("jog")?, 12.0 / 34.0), (clip("run")?, 7.0 / 26.0)];
        let mut mask = JointMask::nothing(skeleton.len());
        mask.set(hips, 1.0);
        mask.set_subtree(skeleton, spine, 1.0);
        Some(Self { clips, forward_gaits, mask })
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct GroundFootwork {
    started: f64,
    support: usize,
    action: Action,
}

#[derive(Clone, Copy, Debug)]
enum Action {
    Brake { remaining: f32, speed: f32 },
    Launch { speed: f32 },
}

impl GroundFootwork {
    fn elapsed(self, now: f64) -> f32 {
        (now - self.started) as f32
    }

    pub(super) fn waiting_for_heel(self, now: f64) -> bool {
        matches!(self.action, Action::Launch { .. }) && self.elapsed(now) < LAUNCH_CATCH
    }

    pub(super) fn write_canonical(self, out: &mut super::canonical::Canonical<'_, impl FnMut(&[u8])>) {
        out.f64(self.started);
        out.len(self.support);
        out.flag(matches!(self.action, Action::Launch { .. }));
        match self.action {
            Action::Brake { remaining, speed } => out.floats(&[remaining, speed]),
            Action::Launch { speed } => out.f32(speed),
        }
    }
}

impl GraphPlayer {
    pub(super) fn update_ground_footwork(
        &mut self,
        input: &GraphInput,
        drawn: f64,
        support: Option<usize>,
    ) -> bool {
        let motion = input.locomotion;
        let can_play = self.graph.footwork.is_some()
            && matches!(self.graph.states[usize::from(self.layers.state)].motion, Motion::Blend(_))
            && motion.grounded
            && !motion.preparing_jump
            && !motion.landed
            && input.combat.is_none()
            && !input.defeated
            && (motion.speed < 0.2 || self.layers.math.cos(motion.direction) > 0.878);
        if !can_play {
            self.footwork = None;
            return false;
        }
        let Some(previous) = self.key.map(|key| key.input) else { return false };
        let old = previous.locomotion;
        let speed = motion.speed;
        let dt = (input.ticks - drawn) as f32;
        let active = self.footwork;
        let braking = active.is_some_and(|phrase| matches!(phrase.action, Action::Brake { .. }));
        if braking && self.gather_requested {
            return false;
        }
        let from_ground = old.grounded && !old.preparing_jump && previous.combat.is_none();
        let loss = old.speed - speed;
        let next = if self.gather_requested && from_ground && old.speed >= 0.56 && loss > 0.19 * dt {
            // The first braking step has already moved the root. Solve the remaining distance
            // from its observed rate of change, including different controller profiles.
            let remaining = (speed * speed * dt / (120.0 * loss)).clamp(0.0, 0.57);
            Some(Action::Brake { remaining, speed: old.speed })
        } else if !self.gather_requested
            && from_ground
            && speed - old.speed > 0.12 * dt
            && (old.speed < 0.40 || braking && old.speed < 2.25)
        {
            Some(Action::Launch { speed })
        } else {
            None
        };
        if let Some((action, support)) = next.zip(support.filter(|&side| side < 2)).filter(|_| dt > 0.0) {
            self.gather_started = None;
            if matches!(action, Action::Launch { .. }) {
                // Set up the receiving upper-body pose at launch, while the outgoing body
                // can still carry through inertia. A walk leaves some of that gait visible.
                self.layers.phase = (1 - support) as f32 * 0.5;
            }
            self.footwork = Some(GroundFootwork { started: drawn, support, action });
            return true;
        }
        self.footwork = active.and_then(|mut phrase| match phrase.action {
            Action::Brake { .. } => (self.gather_requested || speed < 0.25).then_some(phrase),
            Action::Launch { .. } => {
                phrase.action = Action::Launch { speed };
                (phrase.elapsed(input.ticks) < LAUNCH_LIMIT && !self.gather_requested).then_some(phrase)
            }
        });
        false
    }

    pub(super) fn launch_phase(&mut self, input: &GraphInput, drawn: f64) -> Option<f64> {
        let phrase = self.footwork?;
        let catch = phrase.started + f64::from(LAUNCH_CATCH);
        if !matches!(phrase.action, Action::Launch { .. }) || !(drawn < catch && input.ticks >= catch) {
            return None;
        }
        self.layers.velocity = velocity_of(&input.locomotion, self.layers.math);
        self.layers.velocity_rate = sapphire_math::Vec2::ZERO;
        // All three forward cycles begin at a left heel strike. The phase held through
        // the launch now advances with the selected cycle's own stance and flight durations.
        Some(catch)
    }

    pub(crate) fn ground_foot_contact(&self) -> Option<FootContact> {
        let phrase = self.footwork?;
        let elapsed = phrase.elapsed(self.drawn?).max(0.0);
        let support = phrase.support;
        Some(match phrase.action {
            Action::Brake { remaining, speed } => FootContact::Brake { elapsed, support, remaining, speed },
            Action::Launch { speed } => {
                let Motion::Blend(space) = &self.graph.states[usize::from(self.layers.state)].motion else {
                    return None;
                };
                let binding = self.graph.footwork.as_ref()?;
                let mut total = 0.0;
                let mut toe_off = 0.0;
                for (sample, &weight) in space.samples.iter().zip(&self.layers.blend) {
                    if let Some((_, phase)) =
                        binding.forward_gaits.iter().find(|(clip, _)| *clip == sample.clip)
                    {
                        total += weight;
                        toe_off += weight * phase;
                    }
                }
                let toe_off = if total > 0.0 { toe_off / total } else { binding.forward_gaits[0].1 };
                let gait = (self.layers.phase - (1 - support) as f32 * 0.5).rem_euclid(1.0);
                FootContact::Launch { elapsed, support, speed, gait, toe_off }
            }
        })
    }
}

pub(super) fn write_body(
    graph: &GraphBinding,
    clips: &(impl ClipSource + ?Sized),
    pose: &CharacterPose,
    phrase: Option<GroundFootwork>,
    moment: Moment,
    math: PoseMath,
    scratch: &mut [Transform],
    target: &mut Frame,
) {
    let Some((phrase, binding)) = phrase.zip(graph.footwork.as_ref()) else { return };
    let elapsed = phrase.elapsed(moment.ticks);
    let (index, weight) = match phrase.action {
        Action::Brake { speed, .. } => {
            let vigor = smoothstep(0.0, 5.4, speed);
            (0, (0.45 + 0.55 * vigor) * smoothstep(24.0, BRAKE_SETTLED, elapsed))
        }
        Action::Launch { speed } => {
            let vigor = smoothstep(1.8, 4.5, speed);
            (1, (0.52 + 0.48 * vigor) * smoothstep(17.0, 10.0, elapsed))
        }
    };
    if weight > 0.0
        && let Some(clip) = graph.clip(clips, binding.clips[index][phrase.support])
    {
        let position = clip_position(clip, f64::from(elapsed)) as f32;
        sample::sample(clip, position, pose.rest(), scratch);
        blend_with_math(&mut target.joints, scratch, weight, Some(&binding.mask), math);
        for track in clip.weights.iter() {
            if let Some(shown) = target.weights.get_mut(usize::from(track.target.to_native())) {
                *shown += weight * (clip.weight(track, position) - *shown);
            }
        }
    }
}
