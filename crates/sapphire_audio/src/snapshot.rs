//! The perfect-dodge snapshot (spec §5.15): pitch and a low-pass filter driven by the world's time
//! scale. Slow motion sounds slow, so the player feels the dodge land before they read it.
//!
//! Plain numbers in, plain numbers out: a test drives it frame by frame with no clock.

use crate::backend::BusParams;

/// The time scale's reach over a mix. The defaults are a starting point for M2's reviews.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotTuning {
    /// The time scale at and below which the snapshot is at its deepest; 1 and above, it is gone.
    pub floor_scale: f32,
    /// World voices' pitch at the deepest.
    pub floor_pitch: f32,
    /// World buses' low-pass cutoff at the deepest, in hertz.
    pub floor_low_pass_hz: f32,
    /// Seconds for a dip to go about two thirds of the way in.
    pub attack: f32,
    /// Seconds for it to go about two thirds of the way back out.
    pub release: f32,
}

impl Default for SnapshotTuning {
    fn default() -> Self {
        // The sandbox's perfect-dodge slow motion is 0.2×, which reaches the floor.
        Self { floor_scale: 0.2, floor_pitch: 0.75, floor_low_pass_hz: 900.0, attack: 0.03, release: 0.25 }
    }
}

/// The cutoff from which a dip starts, above anything heard, in hertz.
const OPEN_HZ: f32 = 20_000.0;

/// Below this depth the snapshot counts as gone, and the filter comes out.
const GONE: f32 = 0.002;

/// What the snapshot does to the mix this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SnapshotMix {
    /// World voices' pitch multiplier.
    pub pitch: f32,
    /// World buses' parameters.
    pub bus: BusParams,
}

impl SnapshotMix {
    /// No snapshot.
    pub const NEUTRAL: Self = Self { pitch: 1.0, bus: BusParams::OPEN };
}

/// The snapshot's state: how deep the mix is in it, smoothed over real time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockSnapshot {
    tuning: SnapshotTuning,
    depth: f32,
}

impl ClockSnapshot {
    pub fn new(tuning: SnapshotTuning) -> Self {
        Self { tuning, depth: 0.0 }
    }

    pub fn tuning(&self) -> SnapshotTuning {
        self.tuning
    }

    /// Moves the snapshot `dt` real seconds toward where `time_scale` puts it, and returns the mix.
    /// A time scale that isn't a number counts as normal speed.
    pub fn step(&mut self, time_scale: f32, dt: f32) -> SnapshotMix {
        let target = self.target_depth(time_scale);
        let time_constant = if target > self.depth { self.tuning.attack } else { self.tuning.release };
        let share = if dt > 0.0 { 1.0 - (-dt / time_constant.max(1e-4)).exp() } else { 0.0 };
        self.depth += (target - self.depth) * share;
        if target == 0.0 && self.depth < GONE {
            self.depth = 0.0;
        }
        self.mix()
    }

    /// The mix at the present depth.
    pub fn mix(&self) -> SnapshotMix {
        if self.depth == 0.0 {
            return SnapshotMix::NEUTRAL;
        }
        // Log-spaced, as pitch and cutoff are heard.
        let toward = |open: f32, floor: f32| open * (floor / open).powf(self.depth);
        SnapshotMix {
            pitch: toward(1.0, self.tuning.floor_pitch),
            bus: BusParams { low_pass_hz: toward(OPEN_HZ, self.tuning.floor_low_pass_hz) },
        }
    }

    fn target_depth(&self, time_scale: f32) -> f32 {
        if !time_scale.is_finite() {
            return 0.0;
        }
        ((1.0 - time_scale) / (1.0 - self.tuning.floor_scale).max(1e-4)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 60.0;

    fn settled(snapshot: &mut ClockSnapshot, scale: f32) -> SnapshotMix {
        let mut mix = SnapshotMix::NEUTRAL;
        for _ in 0..600 {
            mix = snapshot.step(scale, FRAME);
        }
        mix
    }

    #[test]
    fn normal_speed_leaves_the_mix_alone() {
        let mut snapshot = ClockSnapshot::default();
        assert_eq!(snapshot.step(1.0, FRAME), SnapshotMix::NEUTRAL);
        // Faster than normal is no snapshot either, nor is what isn't a number.
        assert_eq!(snapshot.step(2.0, FRAME), SnapshotMix::NEUTRAL);
        assert_eq!(snapshot.step(f32::NAN, FRAME), SnapshotMix::NEUTRAL);
    }

    #[test]
    fn the_floor_scale_reaches_the_floor_and_below_stays_there() {
        let tuning = SnapshotTuning::default();
        let mut snapshot = ClockSnapshot::new(tuning);
        let floor = settled(&mut snapshot, tuning.floor_scale);
        assert!((floor.pitch - tuning.floor_pitch).abs() < 1e-3, "{floor:?}");
        assert!((floor.bus.low_pass_hz - tuning.floor_low_pass_hz).abs() < 1.0, "{floor:?}");
        // Hitstop-deep slow motion, or a paused world: no deeper.
        let below = settled(&mut snapshot, 0.0);
        assert!((below.pitch - tuning.floor_pitch).abs() < 1e-3, "{below:?}");
    }

    #[test]
    fn a_shallower_slowdown_dips_part_of_the_way() {
        let mut snapshot = ClockSnapshot::default();
        let mix = settled(&mut snapshot, 0.6);
        let tuning = SnapshotTuning::default();
        assert!(mix.pitch > tuning.floor_pitch && mix.pitch < 1.0, "{mix:?}");
        assert!(mix.bus.low_pass_hz > tuning.floor_low_pass_hz && mix.bus.low_pass_hz < OPEN_HZ, "{mix:?}");
    }

    #[test]
    fn it_goes_in_fast_and_comes_out_slowly_and_exactly() {
        let tuning = SnapshotTuning::default();
        let mut snapshot = ClockSnapshot::new(tuning);
        // After 3 frames (50 ms) it is well on its way.
        for _ in 0..3 {
            snapshot.step(tuning.floor_scale, FRAME);
        }
        let in_quickly = snapshot.mix();
        assert!(in_quickly.pitch < 0.82, "{in_quickly:?}");
        settled(&mut snapshot, tuning.floor_scale);
        // 3 frames after the world speeds up it has hardly moved.
        for _ in 0..3 {
            snapshot.step(1.0, FRAME);
        }
        let out_slowly = snapshot.mix();
        assert!(out_slowly.pitch < 0.8, "{out_slowly:?}");
        // And in the end the filter comes all the way out, rather than approaching forever.
        assert_eq!(settled(&mut snapshot, 1.0), SnapshotMix::NEUTRAL);
    }

    #[test]
    fn no_time_means_no_change() {
        let mut snapshot = ClockSnapshot::default();
        assert_eq!(snapshot.step(0.2, 0.0), SnapshotMix::NEUTRAL);
    }
}
