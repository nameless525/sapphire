//! The end of the mix (M6 U10): the player's [`DynamicRange`], then a look-ahead limiter holding the true peak to
//! [`LIMITER_CEILING_DB`], both on the main track after the fold to mono, so nothing reaches the device past them.
//!
//! - **The limiter** hears each frame's true peak 4× oversampled ([`TruePeak`], the meter's own interpolator), so
//!   what a codec or a converter rebuilds between samples stays under the ceiling too, not only the samples. The
//!   gain each frame needs is held over the look-ahead, let back up over [`LIMITER_RELEASE`], and averaged over the
//!   look-ahead again, so it reaches what a peak needs, in a straight line, exactly as the peak plays: the
//!   signal waits [`Master::latency_frames`] for it, [`LIMITER_LOOK_AHEAD`] and the interpolator's half-length
//!   (about 2.1 ms at 48 kHz). Under the ceiling its gain is exactly 1, and the signal leaves as it came, late.
//! - **Dynamic range** compresses the whole mix toward the output profile's loudness: above a threshold under it,
//!   by a ratio, with a gain that leaves a mix at the profile's loudness where it was, so the quiet parts come up
//!   and the loud ones down. It follows the mix's loudness as the meter hears it, a mean square of both channels
//!   K-weighted, quick to rise and slow to fall, so it moves with the loudness of seconds rather than of each hit,
//!   which the limiter catches. `Wide` leaves the mix as it is.
//!
//! Everything is made before the mixer runs, for the highest rate a device gives ([`MAX_RATE`]), so neither a
//! buffer nor a change of rate allocates.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::time::Duration;

use kira::Frame;
use kira::effect::{Effect, EffectBuilder};
use kira::info::Info;
use sapphire_assets::cue::SILENCE_DB;

use crate::loudness::{KWeighting, TruePeak, lufs};

/// The most true peak the mix reaches, in dB under full scale (dBTP): the loudness decision's −1.
pub const LIMITER_CEILING_DB: f32 = -1.0;

/// How far ahead the limiter hears a peak: how long its gain takes to reach what one needs.
pub const LIMITER_LOOK_AHEAD: Duration = Duration::from_millis(2);

/// How long the limiter's gain takes to come about two thirds of the way back once a peak has passed.
pub const LIMITER_RELEASE: Duration = Duration::from_millis(80);

/// What the limiter aims under its ceiling: a gain still moving between two samples bends the points between them
/// by a little more than the interpolator can know.
const CEILING_MARGIN_DB: f32 = 0.1;

/// The highest sample rate the master's buffers hold the whole look-ahead for. Its filters and time constants are
/// made at any rate; a faster device's look-ahead is [`MAX_RATE`]'s frames, so shorter in time.
pub const MAX_RATE: u32 = 192_000;

/// The most frames of look-ahead, at [`MAX_RATE`].
const MAX_LOOK_AHEAD: usize = (MAX_RATE as u64 * LIMITER_LOOK_AHEAD.as_millis() as u64 / 1_000) as usize;

/// Frames of look-ahead at `rate`: [`LIMITER_LOOK_AHEAD`]'s, at most [`MAX_LOOK_AHEAD`].
fn look_ahead_frames(rate: u32) -> usize {
    let frames = (u64::from(rate) * LIMITER_LOOK_AHEAD.as_micros() as u64 / 1_000_000) as usize;
    frames.clamp(1, MAX_LOOK_AHEAD)
}

/// Where a smoothed gain is close enough to its goal to be it: f32's one-pole steps stall short of it, so without
/// this a gain that had moved once would never be exactly 1 again (the limiter's release stalls about 1.1e-4
/// short). The step it takes is under 0.005 dB.
const SNAP: f32 = 5e-4;

/// How much of the mix's loudness range the player hears (M6 U10), a setting saved with the others: all of it, a
/// little less, or a narrow one for a quiet room at night. Relative to the output profile's loudness, so it means
/// the same on a phone and at home.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DynamicRange {
    /// The mix as it is.
    Wide,
    /// Gently narrower: 2:1 above 8 dB under the profile's loudness.
    #[default]
    Standard,
    /// Narrow: 8:1 above 16 dB under it, the quiet parts 14 dB up.
    Night,
}

impl DynamicRange {
    /// Every one, from the widest.
    pub const ALL: [Self; 3] = [Self::Wide, Self::Standard, Self::Night];

    /// How far under the profile's loudness the compression starts, in dB, and its ratio: none for `Wide`.
    #[must_use]
    pub const fn curve(self) -> Option<(f32, f32)> {
        match self {
            Self::Wide => None,
            Self::Standard => Some((8.0, 2.0)),
            Self::Night => Some((16.0, 8.0)),
        }
    }

    const fn index(self) -> u8 {
        match self {
            Self::Wide => 0,
            Self::Standard => 1,
            Self::Night => 2,
        }
    }

    const fn of_index(index: u8) -> Self {
        match index {
            0 => Self::Wide,
            1 => Self::Standard,
            _ => Self::Night,
        }
    }
}

/// How long the dynamic range averages the mix's mean square over, before it follows it.
const RANGE_MEAN_SECONDS: f32 = 0.05;
/// How quickly the dynamic range's level rises toward a louder mix, and falls toward a quieter one: the time each
/// takes to go about two thirds of the way.
const RANGE_RISE_SECONDS: f32 = 0.3;
const RANGE_FALL_SECONDS: f32 = 1.5;
/// How quickly the dynamic range's gain follows its curve, so a change of setting or profile doesn't click.
const RANGE_GAIN_SECONDS: f32 = 0.02;

/// The coefficient of a one-pole smoother that goes about two thirds of the way in `seconds` at `rate`.
fn one_pole(seconds: f32, rate: u32) -> f32 {
    1.0 - (-1.0 / (seconds * rate as f32).max(1.0)).exp()
}

fn gain_of(db: f32) -> f32 {
    10_f32.powf(db / 20.0)
}

/// The dynamic range's compressor. See the [module docs](self).
#[derive(Clone, Debug)]
struct RangeStage {
    /// Each channel's K-weighting, so the stage hears loudness as the meter does: a score's low end counts for less
    /// than a line's presence.
    weighting: [KWeighting; 2],
    /// The mean square of both channels K-weighted, summed, over the last [`RANGE_MEAN_SECONDS`] or so.
    mean: f32,
    /// The mean, followed quickly up and slowly down.
    level: f32,
    /// The gain playing now, linear.
    gain: f32,
    average: f32,
    rise: f32,
    fall: f32,
    follow: f32,
}

impl RangeStage {
    fn new(rate: u32) -> Self {
        let weighting = [KWeighting::new(f64::from(rate)); 2];
        let mut stage = Self {
            weighting,
            mean: 0.0,
            level: 0.0,
            gain: 1.0,
            average: 0.0,
            rise: 0.0,
            fall: 0.0,
            follow: 0.0,
        };
        stage.set_rate(rate);
        stage
    }

    fn set_rate(&mut self, rate: u32) {
        self.weighting = [KWeighting::new(f64::from(rate)); 2];
        self.average = one_pole(RANGE_MEAN_SECONDS, rate);
        self.rise = one_pole(RANGE_RISE_SECONDS, rate);
        self.fall = one_pole(RANGE_FALL_SECONDS, rate);
        self.follow = one_pole(RANGE_GAIN_SECONDS, rate);
    }

    /// The gain for one frame, under `curve` toward `target_lufs`.
    fn frame(&mut self, left: f32, right: f32, curve: Option<(f32, f32)>, target_lufs: f32) -> f32 {
        // Averaged evenly first: a follower quicker up than down, on each sample, would sit over a tone's mean.
        let left = self.weighting[0].filter(f64::from(left)) as f32;
        let right = self.weighting[1].filter(f64::from(right)) as f32;
        self.mean += (left * left + right * right - self.mean) * self.average;
        let toward = if self.mean > self.level { self.rise } else { self.fall };
        self.level += (self.mean - self.level) * toward;
        // Silence decays them toward subnormals, slow on some processors; they mean nothing that far down.
        if self.level < 1e-20 {
            (self.mean, self.level) = (self.mean.max(0.0), 0.0);
        }
        if self.mean < 1e-20 {
            self.mean = 0.0;
        }
        let wanted = match curve {
            None => 1.0,
            Some((below, ratio)) => {
                // BS.1770's loudness of the followed mean square, as the meter reads a block's.
                let level_db = lufs(f64::from(self.level.max(1e-12))) as f32;
                let threshold = target_lufs - below;
                let lift = below * (1.0 - 1.0 / ratio);
                let over = (level_db - threshold).max(0.0);
                gain_of(lift - over * (1.0 - 1.0 / ratio))
            }
        };
        self.gain += (wanted - self.gain) * self.follow;
        // Settled, it's exactly what's wanted, so `Wide` leaves the mix sample for sample.
        if (self.gain - wanted).abs() < SNAP {
            self.gain = wanted;
        }
        self.gain
    }
}

/// The limiter. See the [module docs](self).
#[derive(Clone, Debug)]
struct Limiter {
    peaks: TruePeak,
    /// The most a frame may reach, linear, with the margin.
    ceiling: f32,
    /// Frames of look-ahead at this rate.
    look_ahead: usize,
    release: f32,
    /// The gain each recent frame needs, as a run of rising minima: index and gain, oldest first, from `held_start`.
    held: Box<[(u64, f32)]>,
    held_start: usize,
    held_len: usize,
    /// The released gain of the last `look_ahead` frames, as a ring from `averaged_next`, and their sum.
    averaged: Box<[f32]>,
    averaged_next: usize,
    averaged_sum: f64,
    released: f32,
    /// The signal, waiting for its gain: a ring of `delay` frames from `delayed_next`.
    delayed: Box<[[f32; 2]]>,
    delayed_next: usize,
    /// Frames the peaks' interpolator has been given.
    frames: u64,
}

impl Limiter {
    fn new(rate: u32) -> Self {
        let mut limiter = Self {
            peaks: TruePeak::new(),
            ceiling: gain_of(LIMITER_CEILING_DB - CEILING_MARGIN_DB),
            look_ahead: 1,
            release: 0.0,
            // A frame's need goes in before the oldest leaves, so the run can be one longer than the window.
            held: vec![(0, 1.0); MAX_LOOK_AHEAD + 1].into_boxed_slice(),
            held_start: 0,
            held_len: 0,
            averaged: vec![1.0; MAX_LOOK_AHEAD].into_boxed_slice(),
            averaged_next: 0,
            averaged_sum: 0.0,
            released: 1.0,
            delayed: vec![[0.0; 2]; MAX_LOOK_AHEAD + TruePeak::DELAY].into_boxed_slice(),
            delayed_next: 0,
            frames: 0,
        };
        limiter.set_rate(rate);
        limiter
    }

    /// Starts over at `rate`, with nothing playing.
    fn set_rate(&mut self, rate: u32) {
        self.look_ahead = look_ahead_frames(rate);
        self.release = one_pole(LIMITER_RELEASE.as_secs_f32(), rate);
        self.peaks.clear();
        (self.held_start, self.held_len) = (0, 0);
        self.averaged.fill(1.0);
        (self.averaged_next, self.averaged_sum, self.released) = (0, self.look_ahead as f64, 1.0);
        self.delayed.fill([0.0; 2]);
        (self.delayed_next, self.frames) = (0, 0);
    }

    /// How many frames late the signal leaves.
    fn delay(&self) -> usize {
        self.look_ahead - 1 + TruePeak::DELAY
    }

    /// Takes a frame in, and gives the one [`Self::delay`] frames before it, limited.
    fn frame(&mut self, left: f32, right: f32) -> [f32; 2] {
        // What the frame `TruePeak::DELAY` back needs to stay under the ceiling.
        let peak = self.peaks.push(left, right);
        let needs = if peak > self.ceiling { self.ceiling / peak } else { 1.0 };
        let index = self.frames;
        self.frames += 1;

        // The least of what the last `look_ahead` frames need: a run of rising minima, oldest first.
        let capacity = self.held.len();
        while self.held_len > 0 && self.held[(self.held_start + self.held_len - 1) % capacity].1 >= needs {
            self.held_len -= 1;
        }
        self.held[(self.held_start + self.held_len) % capacity] = (index, needs);
        self.held_len += 1;
        if self.held[self.held_start].0 + self.look_ahead as u64 <= index {
            self.held_start = (self.held_start + 1) % capacity;
            self.held_len -= 1;
        }
        let held = self.held[self.held_start].1;

        // Down at once, back up over the release, never above what's held.
        self.released =
            if held < self.released { held } else { self.released + (held - self.released) * self.release };
        if (held - self.released).abs() < SNAP {
            self.released = held;
        }

        // Averaged over the look-ahead, so it ramps down to a peak in a straight line and never past what it needs.
        let oldest = std::mem::replace(&mut self.averaged[self.averaged_next], self.released);
        self.averaged_sum += f64::from(self.released) - f64::from(oldest);
        self.averaged_next = (self.averaged_next + 1) % self.look_ahead;
        let gain = if self.averaged_sum >= self.look_ahead as f64 {
            1.0
        } else {
            (self.averaged_sum / self.look_ahead as f64) as f32
        };

        let delay = self.delay();
        let [l, r] = std::mem::replace(&mut self.delayed[self.delayed_next], [left, right]);
        self.delayed_next = (self.delayed_next + 1) % delay;
        if gain == 1.0 { [l, r] } else { [l * gain, r * gain] }
    }

    /// Adds the ring's values up again, so the running sum can't drift from them over a long session.
    fn resum(&mut self) {
        self.averaged_sum = self.averaged[..self.look_ahead].iter().map(|&gain| f64::from(gain)).sum();
    }
}

/// The master's stages, as plain numbers: what the main track's effect runs and the costs test times. The player's
/// [`DynamicRange`] toward the profile's loudness, then a look-ahead limiter holding the true peak to
/// [`LIMITER_CEILING_DB`], its gain held over [`LIMITER_LOOK_AHEAD`] and let back up over [`LIMITER_RELEASE`]; the
/// signal leaves [`Master::latency_frames`] late.
#[derive(Clone, Debug)]
pub struct Master {
    range: RangeStage,
    limiter: Limiter,
    rate: u32,
    /// The player's overall volume, linear, and the gain playing it now: after everything, so neither the range
    /// nor the limiter hears it, and the slider moves what leaves by exactly its decibels.
    volume: f32,
    volume_now: f32,
    volume_follow: f32,
}

impl Master {
    /// The stages at `rate`, with nothing playing. Allocates their buffers, for [`MAX_RATE`].
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1);
        let volume_follow = one_pole(RANGE_GAIN_SECONDS, rate);
        Self {
            range: RangeStage::new(rate),
            limiter: Limiter::new(rate),
            rate,
            volume: 1.0,
            volume_now: 1.0,
            volume_follow,
        }
    }

    /// Starts over at `rate`, with nothing playing. Allocates nothing.
    pub fn set_rate(&mut self, rate: u32) {
        self.rate = rate.max(1);
        self.range.set_rate(self.rate);
        self.limiter.set_rate(self.rate);
        self.volume_follow = one_pole(RANGE_GAIN_SECONDS, self.rate);
        self.volume_now = self.volume;
    }

    /// The player's overall volume (M4 A2), in dB: the master's last gain, eased to over 20 ms. At or under
    /// [`SILENCE_DB`] it's silent.
    pub fn set_volume_db(&mut self, volume_db: f32) {
        self.volume =
            if volume_db.is_finite() && volume_db > SILENCE_DB { gain_of(volume_db.min(0.0)) } else { 0.0 };
    }

    /// How many frames late the mix leaves: the limiter's look-ahead and its interpolator's half-length.
    #[must_use]
    pub fn latency_frames(&self) -> usize {
        self.limiter.delay()
    }

    /// [`Self::latency_frames`] in seconds.
    #[must_use]
    pub fn latency(&self) -> Duration {
        Duration::from_secs_f64(self.latency_frames() as f64 / f64::from(self.rate))
    }

    /// Runs interleaved stereo `samples` through, in place, under `range` toward `target_lufs`.
    /// A sample that isn't a finite number comes in as silence, so it can't stay in the filters for good.
    pub fn process(&mut self, samples: &mut [f32], range: DynamicRange, target_lufs: f32) {
        let curve = range.curve();
        for frame in samples.as_chunks_mut::<2>().0 {
            let [left, right] = frame.map(|sample| if sample.is_finite() { sample } else { 0.0 });
            let gain = self.range.frame(left, right, curve, target_lufs);
            let [left, right] = self.limiter.frame(left * gain, right * gain);
            self.volume_now += (self.volume - self.volume_now) * self.volume_follow;
            if (self.volume - self.volume_now).abs() < SNAP {
                self.volume_now = self.volume;
            }
            *frame = if self.volume_now == 1.0 {
                [left, right]
            } else {
                [left, right].map(|s| s * self.volume_now)
            };
        }
        self.limiter.resum();
    }
}

/// What the main thread tells the master: the dynamic range, and the profile's loudness it's relative to.
#[derive(Debug)]
pub(crate) struct MasterControl {
    range: AtomicU8,
    target_lufs: AtomicU32,
    /// The player's overall volume in dB, as bits.
    volume_db: AtomicU32,
    /// The mixer's rate, as the effect last heard it, for the latency.
    rate: AtomicU32,
}

impl MasterControl {
    pub(crate) fn new(range: DynamicRange, target_lufs: f32) -> Self {
        Self {
            range: AtomicU8::new(range.index()),
            target_lufs: AtomicU32::new(target_lufs.to_bits()),
            volume_db: AtomicU32::new(0.0_f32.to_bits()),
            rate: AtomicU32::new(0),
        }
    }

    pub(crate) fn set_volume_db(&self, volume_db: f32) {
        self.volume_db.store(volume_db.to_bits(), Ordering::Relaxed);
    }

    pub(crate) fn set_range(&self, range: DynamicRange) {
        self.range.store(range.index(), Ordering::Relaxed);
    }

    pub(crate) fn set_target(&self, target_lufs: f32) {
        self.target_lufs.store(target_lufs.to_bits(), Ordering::Relaxed);
    }

    /// The delay the master adds, at the rate the mixer runs at: none before it has run.
    pub(crate) fn latency(&self) -> Duration {
        match self.rate.load(Ordering::Relaxed) {
            0 => Duration::ZERO,
            rate => Master::latency_at(rate),
        }
    }
}

impl Master {
    /// [`Self::latency`] at `rate`, without making the buffers.
    #[must_use]
    pub fn latency_at(rate: u32) -> Duration {
        let rate = rate.max(1);
        let delay = look_ahead_frames(rate) - 1 + TruePeak::DELAY;
        Duration::from_secs_f64(delay as f64 / f64::from(rate))
    }
}

/// The master as the main track's last effect.
pub(crate) struct MasterEffect {
    master: Master,
    control: Arc<MasterControl>,
    /// A buffer's frames, interleaved, made for kira's whole internal buffer.
    scratch: Vec<f32>,
}

impl Effect for MasterEffect {
    fn init(&mut self, sample_rate: u32, internal_buffer_size: usize) {
        self.master.set_rate(sample_rate);
        self.control.rate.store(sample_rate, Ordering::Relaxed);
        self.scratch = vec![0.0; internal_buffer_size * 2];
    }

    fn on_change_sample_rate(&mut self, sample_rate: u32) {
        self.master.set_rate(sample_rate);
        self.control.rate.store(sample_rate, Ordering::Relaxed);
    }

    fn process(&mut self, input: &mut [Frame], _dt: f64, _info: &Info<'_>) {
        let range = DynamicRange::of_index(self.control.range.load(Ordering::Relaxed));
        let target = f32::from_bits(self.control.target_lufs.load(Ordering::Relaxed));
        self.master.set_volume_db(f32::from_bits(self.control.volume_db.load(Ordering::Relaxed)));
        // kira never gives a buffer longer than the one it made the effect for; any longer goes in pieces.
        for frames in input.chunks_mut((self.scratch.len() / 2).max(1)) {
            let scratch = &mut self.scratch[..frames.len() * 2];
            for (frame, pair) in frames.iter().zip(scratch.as_chunks_mut::<2>().0) {
                (pair[0], pair[1]) = (frame.left, frame.right);
            }
            self.master.process(scratch, range, target);
            for (frame, pair) in frames.iter_mut().zip(scratch.as_chunks::<2>().0) {
                (frame.left, frame.right) = (pair[0], pair[1]);
            }
        }
    }
}

/// Builds a [`MasterEffect`] that follows its control.
pub(crate) struct MasterBuilder(pub(crate) Arc<MasterControl>);

impl EffectBuilder for MasterBuilder {
    type Handle = ();

    fn build(self) -> (Box<dyn Effect>, ()) {
        // The rate is the device's, given in `init`; a scratch of a buffer's length is made there too.
        (Box::new(MasterEffect { master: Master::new(48_000), control: self.0, scratch: Vec::new() }), ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loudness::measure;

    const RATE: u32 = 48_000;

    fn sine(hz: f32, peak: f32, frames: usize) -> Vec<f32> {
        let step = std::f32::consts::TAU * hz / RATE as f32;
        (0..frames).flat_map(|n| [peak * (step * n as f32).sin(); 2]).collect()
    }

    /// Under the ceiling the limiter is exactly a delay, and `Wide` changes nothing: the mix leaves sample for
    /// sample, [`Master::latency_frames`] late.
    #[test]
    fn under_the_ceiling_the_master_is_a_delay() {
        let input = sine(440.0, 0.5, RATE as usize);
        let mut output = input.clone();
        let mut master = Master::new(RATE);
        master.process(&mut output, DynamicRange::Wide, -24.0);
        let late = master.latency_frames();
        assert_eq!(late, 96 - 1 + 6, "2 ms and the interpolator's half-length at 48 kHz");
        assert_eq!(&output[late * 2..], &input[..input.len() - late * 2]);
        assert!(output[..late * 2].iter().all(|&sample| sample == 0.0));
        assert!((master.latency().as_secs_f64() - 101.0 / 48_000.0).abs() < 1e-9);
        assert_eq!(MasterControl::new(DynamicRange::Wide, -24.0).latency(), Duration::ZERO);
        assert_eq!(Master::latency_at(RATE), master.latency());
    }

    /// Bursts 12 dB over full scale, between samples as well as on them, leave at most −1 dBTP, read by the
    /// meter's own true peak; the quiet between them comes back to unity.
    #[test]
    fn peaks_leave_under_the_ceiling() {
        let mut input = Vec::new();
        for burst in 0..6 {
            // A tone near the top of the band, whose crests fall between samples.
            input.extend(sine(11_000.0 + 1_000.0 * burst as f32, 4.0, RATE as usize / 10));
            input.extend(sine(300.0, 0.25, RATE as usize / 2));
        }
        let mut output = input.clone();
        let mut master = Master::new(RATE);
        master.process(&mut output, DynamicRange::Wide, -24.0);
        let read = measure(&output, RATE);
        assert!(read.true_peak_db <= f64::from(LIMITER_CEILING_DB), "{read:?}");
        assert!(read.true_peak_db > f64::from(LIMITER_CEILING_DB) - 1.0, "it limits, not mutes: {read:?}");
        // The last quiet stretch's end, six releases on: as loud as it went in, within 0.03 dB.
        let late = master.latency_frames() * 2;
        let end = output.len() - 2_000;
        let ratio = output[end - 400..end].iter().map(|s| s.abs()).fold(0.0, f32::max)
            / input[end - 400 - late..end - late].iter().map(|s| s.abs()).fold(0.0, f32::max);
        assert!((ratio - 1.0).abs() < 4e-3, "released to {ratio}");
    }

    /// A lone full-scale click: the gain is down to what it needs by the time it plays, from a straight ramp.
    #[test]
    fn a_click_is_met_by_its_gain_in_time() {
        let mut samples = vec![0.0; 4_000];
        samples[2_000] = 3.0;
        samples[2_001] = -3.0;
        let mut master = Master::new(RATE);
        master.process(&mut samples, DynamicRange::Wide, -24.0);
        let peak = samples.iter().fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        assert!(peak <= gain_of(LIMITER_CEILING_DB), "{peak}");
    }

    /// Night lifts a quiet mix and lowers a loud one toward the profile's loudness; a mix at the profile's loudness
    /// stays near where it was under each.
    #[test]
    fn the_range_pulls_toward_the_profiles_loudness() {
        let level = |peak_db: f32, range| {
            let mut samples = sine(1_000.0, gain_of(peak_db), RATE as usize * 8);
            Master::new(RATE).process(&mut samples, range, -24.0);
            measure(&samples[samples.len() / 2..], RATE).integrated_lufs as f32
        };
        for range in DynamicRange::ALL {
            assert!((level(-24.0, range) + 24.0).abs() < 0.3, "{range:?}");
        }
        let (quiet, loud) = (level(-40.0, DynamicRange::Night), level(-12.0, DynamicRange::Night));
        assert!((quiet + 26.0).abs() < 0.3, "16 dB under, 14 dB up: {quiet}");
        assert!((loud + 22.5).abs() < 0.3, "12 dB over, by 8:1: {loud}");
        assert!((level(-12.0, DynamicRange::Wide) + 12.0).abs() < 0.1);
    }

    fn sine_at(hz: f32, peak: f32, frames: usize, rate: u32) -> Vec<f32> {
        let step = std::f64::consts::TAU * f64::from(hz) / f64::from(rate);
        (0..frames).flat_map(|n| [peak * (step * n as f64).sin() as f32; 2]).collect()
    }

    /// At [`MAX_RATE`] the look-ahead fills its buffers: a bass tone 12 dB over full scale, whose gain needs rise
    /// for longer than the look-ahead on each half cycle, still leaves under the ceiling.
    #[test]
    fn peaks_leave_under_the_ceiling_at_the_highest_rate() {
        let mut samples = sine_at(90.0, 4.0, MAX_RATE as usize / 2, MAX_RATE);
        // Silence after, so the tone's cut-off end plays out through the limiter, not past the render's end.
        samples.extend(vec![0.0; 4 * (MAX_LOOK_AHEAD + TruePeak::DELAY)]);
        let mut master = Master::new(MAX_RATE);
        master.process(&mut samples, DynamicRange::Wide, -24.0);
        assert_eq!(master.limiter.look_ahead, MAX_LOOK_AHEAD);
        let read = measure(&samples, MAX_RATE);
        assert!(read.true_peak_db <= f64::from(LIMITER_CEILING_DB), "{read:?}");
        assert!(read.true_peak_db > f64::from(LIMITER_CEILING_DB) - 1.0, "it limits, not mutes: {read:?}");
    }

    /// The run of rising minima holds, every frame, the least any frame in the look-ahead needs, as a search of the
    /// whole window finds it, at the highest rate and at 48 kHz.
    #[test]
    fn the_held_gain_is_the_windows_least() {
        for rate in [MAX_RATE, RATE] {
            // Over the ceiling in swells longer and shorter than the look-ahead, rising and falling.
            let samples: Vec<f32> = (0..rate as usize / 5)
                .flat_map(|n| {
                    let t = n as f32 / rate as f32;
                    let swell = 0.5 + 2.5 * (std::f32::consts::TAU * 7.0 * t).sin().abs();
                    let tone = (std::f32::consts::TAU * (90.0 + 4_000.0 * (t * 3.0).fract()) * t).sin();
                    [swell * tone; 2]
                })
                .collect();
            let mut limiter = Limiter::new(rate);
            let mut peaks = TruePeak::new();
            let mut needs = Vec::new();
            for (n, frame) in samples.as_chunks::<2>().0.iter().enumerate() {
                let peak = peaks.push(frame[0], frame[1]);
                needs.push(if peak > limiter.ceiling { limiter.ceiling / peak } else { 1.0 });
                limiter.frame(frame[0], frame[1]);
                let window = &needs[(n + 1).saturating_sub(limiter.look_ahead)..];
                let least = window.iter().copied().fold(f32::INFINITY, f32::min);
                assert_eq!(limiter.held[limiter.held_start].1, least, "{rate} Hz, frame {n}");
                assert!(limiter.held_len <= limiter.look_ahead, "{rate} Hz, frame {n}");
            }
        }
    }

    /// One sample that isn't a number comes in as silence: the output stays finite, and the range's gain goes on
    /// as it would have.
    #[test]
    fn a_nan_is_heard_as_silence() {
        let tone = sine(1_000.0, gain_of(-30.0), RATE as usize * 2);
        let mut poisoned = tone.clone();
        poisoned[RATE as usize] = f32::NAN;
        poisoned[RATE as usize + 3] = f32::INFINITY;
        let (mut clean, mut master) = (Master::new(RATE), Master::new(RATE));
        let mut heard = tone.clone();
        clean.process(&mut heard, DynamicRange::Night, -24.0);
        master.process(&mut poisoned, DynamicRange::Night, -24.0);
        assert!(poisoned.iter().all(|sample| sample.is_finite()));
        assert!(
            (master.range.gain - clean.range.gain).abs() < 1e-3,
            "{} and {}",
            master.range.gain,
            clean.range.gain
        );
        assert!(master.range.level.is_finite() && master.range.mean.is_finite());
    }

    /// A gain that has moved settles exactly where it's going: the limiter's back at 1 after a peak, and the range's
    /// at 1 once `Wide` is chosen again, so `Wide` under the ceiling leaves the mix sample for sample again.
    #[test]
    fn moved_gains_settle_exactly() {
        let mut master = Master::new(RATE);
        let mut quiet = sine(1_000.0, gain_of(-40.0), RATE as usize * 4);
        master.process(&mut quiet, DynamicRange::Standard, -24.0);
        assert!(master.range.gain > 1.1, "lifted: {}", master.range.gain);
        let mut loud = sine(1_000.0, 4.0, RATE as usize / 10);
        master.process(&mut loud, DynamicRange::Standard, -24.0);
        assert!(master.limiter.released < 0.5, "limited: {}", master.limiter.released);
        let mut rest = sine(440.0, 0.25, RATE as usize * 2);
        master.process(&mut rest, DynamicRange::Wide, -24.0);
        assert_eq!(master.range.gain, 1.0);
        assert_eq!(master.limiter.released, 1.0);
        let input = sine(440.0, 0.25, RATE as usize / 10);
        let mut output = input.clone();
        master.process(&mut output, DynamicRange::Wide, -24.0);
        let late = master.latency_frames() * 2;
        assert_eq!(&output[late..], &input[..input.len() - late]);
    }

    /// The player's volume is the last gain: under every range it moves what leaves by exactly its decibels, which
    /// neither the range nor the limiter undoes.
    #[test]
    fn the_volume_comes_after_everything() {
        let input = sine(1_000.0, gain_of(-30.0), RATE as usize * 4);
        for range in DynamicRange::ALL {
            let heard = |volume_db: f32| {
                let mut master = Master::new(RATE);
                master.set_volume_db(volume_db);
                let mut samples = input.clone();
                master.process(&mut samples, range, -24.0);
                measure(&samples[samples.len() / 2..], RATE).integrated_lufs
            };
            let full = heard(0.0);
            for volume_db in [-6.02, -13.98] {
                let moved = heard(volume_db) - full;
                assert!((moved - f64::from(volume_db)).abs() < 0.01, "{range:?}: {volume_db} moved {moved}");
            }
        }
        let mut master = Master::new(RATE);
        master.set_volume_db(SILENCE_DB);
        let mut samples = input.clone();
        master.process(&mut samples, DynamicRange::Standard, -24.0);
        assert!(samples[RATE as usize..].iter().all(|&sample| sample == 0.0), "off is silent");
    }
}
