//! A 2D blend space's weights: how much each clip counts at a velocity (spec §5.13).
//!
//! Gradient band interpolation in polar space (Johansen, "Automated Semi-Procedural Animation for
//! Character Locomotion", 2009, §6.3). Each sample gets, from every other, a band that falls from 1
//! at itself to 0 at the other, measured in speed and in direction so that a sample's influence
//! reaches around a circle rather than through the middle of it, and its weight is the lowest of
//! them, normalized. So weights are never negative and sum to 1, every sample is exact at its place
//! with every other at exactly 0, and they change continuously between, with few samples weighing in
//! at once. An idle sample's bands measure how far the input has moved from the origin along
//! each gait's direction. Idle also drops out past the fastest gait, including in directions
//! with no authored sample.
//!
//! A pure function of a position and the samples, allocating nothing.

use sapphire_math::Vec2;

use crate::pose_math::PoseMath;

/// How much a difference of direction counts against a difference of speed. Johansen's value: a
/// quarter turn away counts about as much as twice the speed.
const ANGLE_WEIGHT: f32 = 2.0;

/// Below this speed a position has no direction.
const NO_SPEED: f32 = 1e-5;

/// What's taken off every weight before they're normalized again, so that one too light to see costs
/// no clip's sampling. Off the top rather than a cut, so a weight that reaches it arrives at 0 smoothly
/// and nothing pops, and a weight of 1 is still exactly 1.
const IGNORED_WEIGHT: f32 = 0.05;

/// What's fixed about a sample, worked out once: a position's speed and direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendPoint {
    position: Vec2,
    speed: f32,
}

impl BlendPoint {
    /// A sample at this velocity.
    #[must_use]
    pub fn new(x: f32, z: f32) -> Self {
        let position = Vec2::new(x, z);
        Self { position, speed: position.length() }
    }

    /// Its speed.
    #[must_use]
    pub fn speed(&self) -> f32 {
        self.speed
    }
}

/// The turn from `from` to `to`, in radians, positive in the direction `axis` turns. For a pair exactly
/// opposite, whose `axis` is 0, both ways round lead to the other, so it's the size of the turn. 0 if
/// either is at the origin, which has no direction.
#[inline]
fn turn(from: BlendPoint, to: BlendPoint, axis: f32, math: PoseMath) -> f32 {
    if from.speed < NO_SPEED || to.speed < NO_SPEED {
        return 0.0;
    }
    let angle = math.atan2(from.position.perp_dot(to.position), from.position.dot(to.position));
    if axis == 0.0 {
        angle.abs()
    } else if axis < 0.0 {
        -angle
    } else {
        angle
    }
}

/// Writes `samples`' weights at `at` into `weights`, never negative and summing to 1. `weights` holds
/// one per sample. Allocates nothing; costs the square of the samples.
pub fn blend_weights(samples: &[BlendPoint], at: BlendPoint, weights: &mut [f32]) {
    blend_weights_with_math(samples, at, weights, PoseMath::Legacy);
}

pub(crate) fn blend_weights_with_math(
    samples: &[BlendPoint],
    at: BlendPoint,
    weights: &mut [f32],
    math: PoseMath,
) {
    let outer_speed = samples.iter().map(BlendPoint::speed).fold(NO_SPEED, f32::max);
    let mut total = 0.0;
    for (i, (&sample, weight)) in samples.iter().zip(weights.iter_mut()).enumerate() {
        let mut lowest = f32::INFINITY;
        for (j, &other) in samples.iter().enumerate() {
            if i == j {
                continue;
            }
            // From an idle, only velocity along the other sample's direction leaves its band.
            // A slow retreat would otherwise shrink the blend into every faster gait.
            if sample.speed < NO_SPEED && other.speed >= NO_SPEED {
                let projected = at.position.dot(other.position / other.speed) / other.speed;
                lowest = lowest.min(1.0 - projected.max(at.speed / outer_speed));
                continue;
            }
            // Measured against both's speeds, so a band's shape doesn't depend on the space's scale.
            let scale = 2.0 / (sample.speed + other.speed).max(NO_SPEED);
            // Which way round counts as positive for this pair.
            let axis = sample.position.perp_dot(other.position);
            let band = Vec2::new(
                (other.speed - sample.speed) * scale,
                ANGLE_WEIGHT * turn(sample, other, axis, math),
            );
            let along =
                Vec2::new((at.speed - sample.speed) * scale, ANGLE_WEIGHT * turn(sample, at, axis, math));
            lowest = lowest.min(1.0 - along.dot(band) / band.length_squared().max(f32::MIN_POSITIVE));
        }
        let band = lowest.max(0.0);
        // One sample alone has no others to fall toward.
        *weight = if band.is_finite() { band } else { 1.0 };
        total += *weight;
    }
    if total <= 0.0 {
        // No band reaches here, which only rounding can do: the nearest takes it.
        nearest(samples, at, weights);
        return;
    }
    let mut kept = 0.0;
    for weight in weights.iter_mut().take(samples.len()) {
        *weight = (*weight / total - IGNORED_WEIGHT).max(0.0);
        kept += *weight;
    }
    if kept <= 0.0 {
        nearest(samples, at, weights);
        return;
    }
    for weight in weights.iter_mut().take(samples.len()) {
        *weight /= kept;
    }
}

fn nearest(samples: &[BlendPoint], at: BlendPoint, weights: &mut [f32]) {
    let mut best = (f32::INFINITY, 0);
    for (index, sample) in samples.iter().enumerate() {
        let distance = sample.position.distance_squared(at.position);
        if distance < best.0 {
            best = (distance, index);
        }
    }
    for (index, weight) in weights.iter_mut().enumerate().take(samples.len()) {
        *weight = if index == best.1 { 1.0 } else { 0.0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// As the character's locomotion has it: an idle, walk, run, walking backwards and two strafes.
    fn space() -> [BlendPoint; 6] {
        [
            BlendPoint::new(0.0, 0.0),
            BlendPoint::new(0.0, 2.0),
            BlendPoint::new(0.0, 6.0),
            BlendPoint::new(0.0, -0.95),
            BlendPoint::new(4.78, 2.76),
            BlendPoint::new(-4.78, 2.76),
        ]
    }

    fn weights_at(x: f32, z: f32) -> [f32; 6] {
        let mut weights = [0.0; 6];
        blend_weights(&space(), BlendPoint::new(x, z), &mut weights);
        weights
    }

    #[test]
    fn every_sample_is_exact_at_its_place() {
        for math in [PoseMath::Legacy, PoseMath::Pinned] {
            for (index, sample) in space().into_iter().enumerate() {
                let mut weights = [9.0; 6];
                blend_weights_with_math(&space(), sample, &mut weights, math);
                for (other, &weight) in weights.iter().enumerate() {
                    assert_eq!(
                        weight,
                        if other == index { 1.0 } else { 0.0 },
                        "{math:?} at sample {index}: {weights:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn weights_sum_to_one_and_are_never_negative_everywhere() {
        for x in -40..=40 {
            for z in -40..=40 {
                let weights = weights_at(x as f32 * 0.2, z as f32 * 0.2);
                let sum: f32 = weights.iter().sum();
                assert!((sum - 1.0).abs() < 1e-5, "at ({x}, {z}): {weights:?}");
                assert!(weights.iter().all(|&weight| (0.0..=1.0).contains(&weight)), "{weights:?}");
            }
        }
    }

    #[test]
    fn pinned_weights_are_continuous_through_direction_and_speed_bands() {
        let samples = space();
        for x in -20..=20 {
            for z in -20..=20 {
                let at = BlendPoint::new(x as f32 * 0.3, z as f32 * 0.3);
                let mut weights = [0.0; 6];
                blend_weights_with_math(&samples, at, &mut weights, PoseMath::Pinned);
                assert!((weights.iter().sum::<f32>() - 1.0).abs() < 1e-5);
                assert!(weights.iter().all(|w| (0.0..=1.0).contains(w)));
                let mut neighbour = [0.0; 6];
                let shift = BlendPoint::new(at.position.x + 0.005, at.position.y - 0.005);
                blend_weights_with_math(&samples, shift, &mut neighbour, PoseMath::Pinned);
                assert!(weights.iter().zip(neighbour).all(|(a, b)| (*a - b).abs() < 0.08));
            }
        }
    }

    /// No jump anywhere: a small step of velocity makes a small change of weights.
    #[test]
    fn weights_change_continuously() {
        let step = 0.02;
        let mut worst = 0.0_f32;
        for x in -120..=120 {
            for z in -120..=120 {
                let (x, z) = (x as f32 * 0.05, z as f32 * 0.05);
                let here = weights_at(x, z);
                for there in [weights_at(x + step, z), weights_at(x, z + step)] {
                    for (a, b) in here.iter().zip(&there) {
                        worst = worst.max((a - b).abs());
                    }
                }
            }
        }
        // Across the far side a pair of samples opposite each other has a sharp but continuous ridge.
        assert!(worst < 0.2, "{worst}");
    }

    /// Between two samples on one line, the weights are the plain interpolation.
    #[test]
    fn along_a_line_it_interpolates_by_speed() {
        let line = [BlendPoint::new(0.0, 0.0), BlendPoint::new(0.0, 1.0), BlendPoint::new(0.0, 4.7)];
        let mut weights = [0.0; 3];
        blend_weights(&line, BlendPoint::new(0.0, 0.5), &mut weights);
        assert!((weights[0] - 0.5).abs() < 1e-5 && (weights[1] - 0.5).abs() < 1e-5, "{weights:?}");
        assert_eq!(weights[2], 0.0);
        blend_weights(&line, BlendPoint::new(0.0, 2.85), &mut weights);
        assert!((weights[1] - 0.5).abs() < 1e-5 && (weights[2] - 0.5).abs() < 1e-5, "{weights:?}");
        assert_eq!(weights[0], 0.0);
    }

    #[test]
    fn idle_blends_into_each_direction_at_its_own_speed() {
        let samples = space();
        for index in [1, 3, 4, 5] {
            let sample = samples[index];
            let mut previous_idle = 1.0;
            for step in 1..=20 {
                let at = sample.position * (step as f32 / 20.0);
                let weights = weights_at(at.x, at.y);
                assert!(weights[0] <= previous_idle + 1e-6, "sample {index}, step {step}: {weights:?}");
                previous_idle = weights[0];
                if step == 10 {
                    // The slower backward gait must not suppress idle in the forward or diagonal rays.
                    if index == 1 || index == 3 {
                        assert!((weights[0] - 0.5).abs() < 1e-5, "{weights:?}");
                        assert!((weights[index] - 0.5).abs() < 1e-5, "{weights:?}");
                    } else {
                        assert!(weights[0] > 0.25 && weights[index] > 0.5, "{weights:?}");
                    }
                }
            }
        }
    }

    /// Past the fastest sample it stays that sample: playback speeds up, the blend doesn't change.
    #[test]
    fn past_the_edge_the_nearest_keeps_it() {
        assert_eq!(weights_at(0.0, 9.0)[2], 1.0);
        assert!(weights_at(0.0, -5.0)[3] > 0.8, "{:?}", weights_at(0.0, -5.0));
    }

    #[test]
    fn idle_drops_out_past_the_edge_in_uncovered_directions() {
        let samples = [BlendPoint::new(0.0, 0.0), BlendPoint::new(0.0, 2.0)];
        let mut weights = [0.0; 2];
        for at in [BlendPoint::new(3.0, 0.0), BlendPoint::new(0.0, -3.0)] {
            blend_weights(&samples, at, &mut weights);
            assert_eq!(weights, [0.0, 1.0]);
        }
    }

    /// A small space has no ridge to worry about: a single sample takes everything.
    #[test]
    fn one_sample_always_counts() {
        let mut weights = [0.0];
        blend_weights(&[BlendPoint::new(0.0, 2.0)], BlendPoint::new(3.0, -1.0), &mut weights);
        assert_eq!(weights, [1.0]);
    }
}
